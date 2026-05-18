//! `skill-pool capture` — the explicit "save this lesson" path.
//!
//! Takes a local directory containing a `SKILL.md` (and any supporting
//! files), bundles it, and uploads as a *draft* — not a published skill.
//! A curator reviews via the web UI and either publishes (assigning a
//! version) or discards.
//!
//! Distinguished from `publish` by:
//!   - No version flag — version is set by the curator at publish time.
//!   - Hits `/v1/drafts`, lands in the inbox.
//!   - Optional `--notes` flag for "why this matters" context.
//!
//! Phase 4 first slice. The async scorer + capturer daemon land later.

use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::client::{CaptureMetadata, Client};
use crate::config::Config;
use crate::install;

pub async fn run(
    cfg: &Config,
    dir: &Path,
    slug_override: Option<&str>,
    notes: Option<&str>,
    extra_tags: &[String],
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

    let mut tags = fm.tags.clone();
    for t in extra_tags {
        if !tags.contains(t) {
            tags.push(t.clone());
        }
    }

    let bundle = install::tar_gz_dir(dir).context("build bundle")?;
    println!(
        "  packing:  {} bytes ({} → draft {})",
        bundle.len(),
        dir.display(),
        slug
    );

    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;

    let metadata = CaptureMetadata {
        slug: &slug,
        origin: "cli",
        notes,
        tags: &tags,
        when_to_use: fm.when_to_use.as_deref(),
    };

    let draft = client.submit_draft(metadata, bundle).await?;
    println!("  draft:    {} ({})", draft.slug, draft.id);
    println!("  status:   {}", draft.status);
    println!();
    println!("Review in the inbox: {}/drafts", reg.url.trim_end_matches('/'));
    Ok(())
}
