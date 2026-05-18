use anyhow::{Context, Result};

use crate::client::Client;
use crate::cmd::ensure;
use crate::config::Config;
use crate::manifest::{self, SkillRef};

pub async fn run(cfg: &Config, slug: &str) -> Result<()> {
    let project_root = manifest::find_project_root().context("locate project root")?;
    let mut mf = manifest::load_in(&project_root).unwrap_or_default();

    // Validate the skill exists on the registry before mutating local files.
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let meta = client.get_skill(slug).await.with_context(|| {
        format!(
            "verify `{slug}` exists on {} for tenant {}",
            reg.url, reg.tenant
        )
    })?;

    let already = mf.skills.iter().any(|s| s.slug == slug);
    if already {
        println!("(already in manifest: {slug}@{})", meta.version);
    } else {
        mf.skills.push(SkillRef {
            slug: slug.to_string(),
            version: "*".to_string(),
            scope: "project".to_string(),
        });
        manifest::save_in(&project_root, &mf)?;
        println!("added: {slug}@{} (manifest updated)", meta.version);
    }

    ensure::run(cfg).await
}
