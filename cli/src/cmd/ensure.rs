use anyhow::{Context, Result};

use crate::client::Client;
use crate::config::Config;
use crate::install::{self, SymlinkResult};
use crate::manifest;

pub async fn run(cfg: &Config) -> Result<()> {
    run_with_quiet(cfg, false).await
}

/// `--quiet` mode suppresses per-skill progress lines. Errors still surface.
/// Used by the direnv hook to stay silent on the happy path.
pub async fn run_with_quiet(cfg: &Config, quiet: bool) -> Result<()> {
    let project_root = manifest::find_project_root().context("locate project root")?;
    let mf = manifest::load_in(&project_root).context("load .skill-pool/manifest.toml")?;

    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let tenant_dir = mf.project.tenant.as_deref().unwrap_or(&reg.tenant);

    if mf.skills.is_empty() {
        if !quiet {
            println!("(manifest has no skills; add some with `skill-pool add <slug>`)");
        }
        return Ok(());
    }

    // Expand the manifest with the transitive dependency closure of each
    // top-level skill. Manifest-declared scope is preserved; transitively-
    // pulled skills inherit their parent's scope. Duplicates collapse —
    // we only install each (slug, version) once.
    let mut work: Vec<InstallTarget> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for skill in &mf.skills {
        if !seen.insert(skill.slug.clone()) {
            continue;
        }
        work.push(InstallTarget {
            slug: skill.slug.clone(),
            version: skill.version.clone(),
            scope: skill.scope.clone(),
            depth: 0,
        });
        // Pull the closure. A non-2xx (e.g. 404 on a forward reference)
        // is reported but doesn't block the rest of the install.
        match client.get_deps(&skill.slug).await {
            Ok(deps) => {
                for d in deps {
                    if !seen.insert(d.slug.clone()) {
                        continue;
                    }
                    work.push(InstallTarget {
                        slug: d.slug,
                        version: if d.version_range.is_empty() {
                            "*".into()
                        } else {
                            d.version_range
                        },
                        scope: skill.scope.clone(),
                        depth: d.depth.max(1) as u32,
                    });
                }
            }
            Err(e) => {
                if !quiet {
                    println!(
                        "  warn:     could not resolve deps of {}: {e}",
                        skill.slug
                    );
                }
            }
        }
    }

    for target in &work {
        let resolved_version = if target.version == "*" {
            match client.get_skill(&target.slug).await {
                Ok(meta) => meta.version,
                Err(e) => {
                    if !quiet {
                        println!(
                            "  warn:     skipping {} (transitive dep, slug not published yet: {e})",
                            target.slug
                        );
                    }
                    continue;
                }
            }
        } else {
            target.version.clone()
        };

        let library_entry = install::library_entry(tenant_dir, &target.slug, &resolved_version)?;
        let target_parent = install::target_for_scope(&project_root, &target.scope)?;
        let indent = if target.depth == 0 { "" } else { "  " };

        if !library_entry.exists() {
            if !quiet {
                println!(
                    "  {indent}fetching: {}@{} → {}",
                    target.slug,
                    resolved_version,
                    library_entry.display()
                );
            }
            let bytes = client.download_bundle(&target.slug).await?;
            install::extract_bundle(&bytes, &library_entry)?;
        } else if !quiet {
            println!("  {indent}cached:   {}@{}", target.slug, resolved_version);
        }

        match install::symlink_into(&library_entry, &target_parent, &target.slug)? {
            SymlinkResult::Created if !quiet => println!(
                "  {indent}link:     {} ({})",
                target.slug,
                target_parent.display()
            ),
            SymlinkResult::Relinked if !quiet => println!(
                "  {indent}relink:   {} ({})",
                target.slug,
                target_parent.display()
            ),
            SymlinkResult::AlreadyOk if !quiet => {
                println!("  {indent}ok:       {}", target.slug)
            }
            _ => {}
        }
    }

    Ok(())
}

/// One concrete skill to install: a manifest entry OR a transitively-
/// pulled dependency. `depth=0` is a top-level manifest entry.
struct InstallTarget {
    slug: String,
    version: String,
    scope: String,
    depth: u32,
}
