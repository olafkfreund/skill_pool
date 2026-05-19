use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::client::{Client, PublishMetadata};
use crate::config::Config;
use crate::install;

pub async fn run(
    cfg: &Config,
    dir: &Path,
    slug_override: Option<&str>,
    version: &str,
    kind: &str,
) -> Result<()> {
    let skill_md = dir.join("SKILL.md");
    if !skill_md.exists() {
        bail!("no SKILL.md found in {}", dir.display());
    }

    let fm = install::read_frontmatter(dir).context("read SKILL.md frontmatter")?;
    let slug = match slug_override {
        Some(s) => s.to_string(),
        None => fm
            .name
            .clone()
            .or_else(|| dir.file_name().map(|n| n.to_string_lossy().into_owned()))
            .ok_or_else(|| anyhow::anyhow!("could not infer slug; pass --slug"))?,
    };

    let bundle = install::tar_gz_dir(dir).context("build bundle")?;
    println!(
        "  packing:  {} bytes ({} → {}@{} [{}])",
        bundle.len(),
        dir.display(),
        slug,
        version,
        kind,
    );

    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;

    // Only forward `kind` over the wire when the caller picked a
    // non-default surface. Keeps the multipart payload byte-identical
    // to the pre-Phase-5 shape on `--kind skill` (the default).
    let kind_override = if kind == "skill" { None } else { Some(kind) };

    let metadata = PublishMetadata {
        slug: &slug,
        version,
        when_to_use: fm.when_to_use.as_deref(),
        tags: &fm.tags,
        kind: kind_override,
    };

    let published = client.publish(metadata, bundle).await?;
    println!("  published: {}@{}", published.slug, published.version);
    println!("  status:    {}", published.status);
    if !published.tags.is_empty() {
        println!("  tags:      {}", published.tags.join(", "));
    }
    Ok(())
}
