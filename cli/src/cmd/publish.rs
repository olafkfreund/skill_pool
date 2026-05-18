use std::path::Path;

use anyhow::{Context, Result};

use crate::config::Config;

pub async fn run(_cfg: &Config, dir: &Path) -> Result<()> {
    let skill_md = dir.join("SKILL.md");
    if !skill_md.exists() {
        anyhow::bail!("no SKILL.md found in {}", dir.display());
    }
    let _raw = std::fs::read_to_string(&skill_md)
        .with_context(|| format!("read {}", skill_md.display()))?;
    // TODO(#3): parse frontmatter, lint, tar+gzip directory, POST multipart.
    anyhow::bail!("`publish` is scaffolded but not yet implemented (issue #3)");
}
