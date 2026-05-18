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

    for skill in &mf.skills {
        let resolved_version = if skill.version == "*" {
            let meta = client.get_skill(&skill.slug).await?;
            meta.version
        } else {
            skill.version.clone()
        };

        let library_entry = install::library_entry(tenant_dir, &skill.slug, &resolved_version)?;
        let target_parent = install::target_for_scope(&project_root, &skill.scope)?;

        if !library_entry.exists() {
            if !quiet {
                println!(
                    "  fetching: {}@{} → {}",
                    skill.slug,
                    resolved_version,
                    library_entry.display()
                );
            }
            let bytes = client.download_bundle(&skill.slug).await?;
            install::extract_bundle(&bytes, &library_entry)?;
        } else if !quiet {
            println!("  cached:   {}@{}", skill.slug, resolved_version);
        }

        match install::symlink_into(&library_entry, &target_parent, &skill.slug)? {
            SymlinkResult::Created if !quiet => {
                println!("  link:     {} ({})", skill.slug, target_parent.display())
            }
            SymlinkResult::Relinked if !quiet => {
                println!("  relink:   {} ({})", skill.slug, target_parent.display())
            }
            SymlinkResult::AlreadyOk if !quiet => println!("  ok:       {}", skill.slug),
            _ => {}
        }
    }

    Ok(())
}
