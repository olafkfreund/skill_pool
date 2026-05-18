use anyhow::{Context, Result};

use crate::client::Client;
use crate::config::Config;
use crate::install::{self, SymlinkResult};
use crate::manifest;

pub async fn run(cfg: &Config) -> Result<()> {
    let project_root = manifest::find_project_root().context("locate project root")?;
    let mf = manifest::load_in(&project_root).context("load .skill-pool/manifest.toml")?;

    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let tenant_dir = mf.project.tenant.as_deref().unwrap_or(&reg.tenant);

    if mf.skills.is_empty() {
        println!("(manifest has no skills; add some with `skill-pool add <slug>`)");
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
            println!(
                "  fetching: {}@{} → {}",
                skill.slug,
                resolved_version,
                library_entry.display()
            );
            let bytes = client.download_bundle(&skill.slug).await?;
            install::extract_bundle(&bytes, &library_entry)?;
        } else {
            println!("  cached:   {}@{}", skill.slug, resolved_version);
        }

        match install::symlink_into(&library_entry, &target_parent, &skill.slug)? {
            SymlinkResult::Created => {
                println!("  link:     {} ({})", skill.slug, target_parent.display())
            }
            SymlinkResult::Relinked => {
                println!("  relink:   {} ({})", skill.slug, target_parent.display())
            }
            SymlinkResult::AlreadyOk => println!("  ok:       {}", skill.slug),
        }
    }

    Ok(())
}
