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
use crate::secret_scan;

pub async fn run(
    cfg: &Config,
    dir: &Path,
    slug_override: Option<&str>,
    notes: Option<&str>,
    extra_tags: &[String],
    allow_secret: bool,
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

    // Pre-POST secret scan. The explicit `capture` path is operator-driven
    // but the same hazard applies: a SKILL.md may have grown a token paste
    // during authoring. `--allow-secret` downgrades this to a warning for
    // skills that legitimately discuss credentials.
    let findings = secret_scan::scan_bundle(&bundle)
        .context("scan bundle for secrets")?;
    if !findings.is_empty() {
        let summary = secret_scan::summarise(&findings);
        if allow_secret {
            println!(
                "  ! secret findings ({}); proceeding under --allow-secret: {}",
                findings.len(),
                summary,
            );
        } else {
            bail!(
                "{} secret finding{} in bundle: {}. Re-run with --allow-secret if these are false positives.",
                findings.len(),
                if findings.len() == 1 { "" } else { "s" },
                summary,
            );
        }
    }

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
