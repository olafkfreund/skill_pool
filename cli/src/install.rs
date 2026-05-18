//! Local installer plumbing: tar a directory, extract a bundle, symlink a
//! library entry into a target. Mirrors `scripts/install.sh` semantics so the
//! Phase 0 install path keeps working in parallel.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use bytes::Bytes;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;

/// Filenames/dirs we never want inside a published bundle.
const TAR_DENYLIST: &[&str] = &[
    ".git",
    ".direnv",
    ".envrc",
    "node_modules",
    "target",
    ".DS_Store",
    "result",
];

/// Build a `.tar.gz` of `dir`. The archive contains the *contents* of `dir`
/// (so `SKILL.md` lives at the archive root). Skips entries in `TAR_DENYLIST`.
pub fn tar_gz_dir(dir: &Path) -> Result<Bytes> {
    if !dir.is_dir() {
        bail!("not a directory: {}", dir.display());
    }
    let canonical = dir.canonicalize()?;
    let mut builder = tar::Builder::new(GzEncoder::new(Vec::new(), Compression::default()));
    builder.follow_symlinks(true);

    for entry in walkdir::WalkDir::new(&canonical)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| !is_denied(e.file_name().to_string_lossy().as_ref()))
    {
        let entry = entry?;
        let path = entry.path();
        let rel = path
            .strip_prefix(&canonical)
            .context("strip prefix while taring")?;

        if entry.file_type().is_dir() {
            builder.append_dir(rel, path)?;
        } else if entry.file_type().is_file() {
            let mut f = fs::File::open(path)?;
            builder.append_file(rel, &mut f)?;
        }
        // Symlinks not preserved on purpose — server's validator wouldn't trust them anyway.
    }
    let gz = builder.into_inner()?;
    let bytes = gz.finish()?;
    Ok(Bytes::from(bytes))
}

fn is_denied(name: &str) -> bool {
    TAR_DENYLIST.contains(&name)
}

/// Read SKILL.md frontmatter from a directory. Returns (name, description, tags).
pub fn read_frontmatter(dir: &Path) -> Result<Frontmatter> {
    let skill_md = dir.join("SKILL.md");
    let raw =
        fs::read_to_string(&skill_md).with_context(|| format!("read {}", skill_md.display()))?;
    parse_frontmatter(&raw)
}

#[derive(Debug, Default, serde::Deserialize)]
#[allow(dead_code)] // `description` is informational; read by future `doctor` checks
pub struct Frontmatter {
    pub name: Option<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn parse_frontmatter(content: &str) -> Result<Frontmatter> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or_else(|| anyhow!("SKILL.md has no YAML frontmatter (expected leading `---`)"))?;
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.find("\n---\r\n"))
        .ok_or_else(|| anyhow!("SKILL.md frontmatter has no closing `---`"))?;
    let yaml = &rest[..end];
    let fm: Frontmatter = serde_yaml::from_str(yaml).context("parse frontmatter")?;
    Ok(fm)
}

/// Extract a `.tar.gz` bundle into `dest`. Creates `dest` if missing. Idempotent
/// in the sense that re-extracting overwrites the same files, but we delete
/// `dest` first to avoid stale entries from a previous version.
pub fn extract_bundle(bundle: &Bytes, dest: &Path) -> Result<()> {
    if dest.exists() {
        fs::remove_dir_all(dest).with_context(|| format!("clear {}", dest.display()))?;
    }
    fs::create_dir_all(dest)?;
    let gz = GzDecoder::new(bundle.as_ref());
    let mut archive = tar::Archive::new(gz);
    archive.set_preserve_permissions(true);
    archive.unpack(dest).context("untar bundle")?;
    Ok(())
}

/// Symlink the library entry at `library_entry` into `target_parent/<slug>`.
///
/// Semantics match `scripts/install.sh`:
///  - If the target is already our symlink → no-op.
///  - If the target is a different symlink → relink (point at new source).
///  - If the target is a non-symlink path → refuse to overwrite (returns Err).
///  - Otherwise create the symlink.
pub fn symlink_into(
    library_entry: &Path,
    target_parent: &Path,
    slug: &str,
) -> Result<SymlinkResult> {
    fs::create_dir_all(target_parent)
        .with_context(|| format!("mkdir -p {}", target_parent.display()))?;
    let dst = target_parent.join(slug);
    let src = library_entry
        .canonicalize()
        .with_context(|| format!("canonicalize {}", library_entry.display()))?;

    let metadata = fs::symlink_metadata(&dst).ok();
    match metadata {
        None => {
            #[cfg(unix)]
            std::os::unix::fs::symlink(&src, &dst)?;
            #[cfg(not(unix))]
            return Err(anyhow!("symlinking is unix-only in Phase 1"));
            Ok(SymlinkResult::Created)
        }
        Some(md) if md.file_type().is_symlink() => {
            let current = fs::read_link(&dst)?;
            if current == src {
                Ok(SymlinkResult::AlreadyOk)
            } else {
                fs::remove_file(&dst)?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(&src, &dst)?;
                Ok(SymlinkResult::Relinked)
            }
        }
        Some(_) => Err(anyhow!(
            "refusing to overwrite non-symlink: {}",
            dst.display()
        )),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum SymlinkResult {
    Created,
    Relinked,
    AlreadyOk,
}

/// Default library directory: `~/.skill-pool/library/<tenant>/<slug>@<version>/`
pub fn library_entry(tenant: &str, slug: &str, version: &str) -> Result<PathBuf> {
    let home = directories::BaseDirs::new()
        .ok_or_else(|| anyhow!("could not determine home dir"))?
        .home_dir()
        .to_path_buf();
    Ok(home
        .join(".skill-pool")
        .join("library")
        .join(tenant)
        .join(format!("{slug}@{version}")))
}

/// Resolve the install target for a scope.
///  - "project" → `<project>/.claude/skills/`
///  - "personal" → `~/.claude/skills/`
pub fn target_for_scope(project_root: &Path, scope: &str) -> Result<PathBuf> {
    match scope {
        "project" => Ok(project_root.join(".claude").join("skills")),
        "personal" => {
            let home = directories::BaseDirs::new()
                .ok_or_else(|| anyhow!("could not determine home dir"))?
                .home_dir()
                .to_path_buf();
            Ok(home.join(".claude").join("skills"))
        }
        other => Err(anyhow!(
            "unknown scope `{other}`; expected project|personal"
        )),
    }
}

/// Smoke test that an extracted library entry actually contains a SKILL.md.
#[allow(dead_code)]
pub fn verify_skill_md(library_entry: &Path) -> Result<()> {
    let p = library_entry.join("SKILL.md");
    if !p.exists() {
        bail!("extracted bundle missing SKILL.md at {}", p.display());
    }
    let mut head = String::new();
    fs::File::open(&p)?.take(8).read_to_string(&mut head)?;
    if !head.starts_with("---") {
        bail!(
            "SKILL.md at {} does not start with frontmatter",
            p.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frontmatter_parses() {
        let raw = "---\nname: foo\ndescription: A test\ntags: [a, b]\n---\nbody\n";
        let fm = parse_frontmatter(raw).expect("ok");
        assert_eq!(fm.name.as_deref(), Some("foo"));
        assert_eq!(fm.description, "A test");
        assert_eq!(fm.tags, vec!["a", "b"]);
    }

    #[test]
    fn frontmatter_requires_leading_delimiter() {
        assert!(parse_frontmatter("no frontmatter").is_err());
    }

    #[test]
    fn frontmatter_requires_closing_delimiter() {
        assert!(parse_frontmatter("---\nname: foo\nno end\n").is_err());
    }
}
