//! Bundle validation: parse SKILL.md frontmatter, enforce length limits,
//! and run a small secret scanner so we never persist obvious leaks.
//!
//! A bundle is a gzipped tar archive containing at minimum a `SKILL.md` at
//! the archive root. Other files may live alongside; they're stored as-is.

use std::io::Read;

use bytes::Bytes;
use flate2::read::GzDecoder;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const MAX_DESCRIPTION_LEN: usize = 1536;
const MAX_BUNDLE_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub struct Frontmatter {
    pub name: Option<String>,
    pub description: String,
    #[serde(default)]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ValidatedBundle {
    pub frontmatter: Frontmatter,
    pub sha256_hex: String,
    pub size_bytes: usize,
}

#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("bundle exceeds {MAX_BUNDLE_BYTES} byte limit")]
    TooLarge,
    #[error("bundle is not a valid gzip + tar archive: {0}")]
    NotAnArchive(String),
    #[error("SKILL.md missing at archive root")]
    MissingSkillMd,
    #[error("SKILL.md has no YAML frontmatter (expected leading `---` block)")]
    MissingFrontmatter,
    #[error("SKILL.md frontmatter failed to parse: {0}")]
    BadFrontmatter(String),
    #[error("description must be 1..={MAX_DESCRIPTION_LEN} characters; got {0}")]
    BadDescriptionLength(usize),
    #[error("forbidden absolute path in SKILL.md body: {0}")]
    AbsolutePath(String),
    #[error("possible secret detected: {0}")]
    Secret(&'static str),
}

pub fn validate(bytes: &Bytes) -> Result<ValidatedBundle, BundleError> {
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge);
    }

    let sha256_hex = {
        let mut h = Sha256::new();
        h.update(bytes);
        hex::encode(h.finalize())
    };

    let skill_md = extract_skill_md(bytes)?;
    let (frontmatter_raw, body) = split_frontmatter(&skill_md)?;
    let fm: Frontmatter = serde_yaml::from_str(frontmatter_raw)
        .map_err(|e| BundleError::BadFrontmatter(e.to_string()))?;

    if fm.description.is_empty() || fm.description.len() > MAX_DESCRIPTION_LEN {
        return Err(BundleError::BadDescriptionLength(fm.description.len()));
    }

    check_absolute_paths(body)?;
    check_secrets(&skill_md)?;

    Ok(ValidatedBundle {
        frontmatter: fm,
        sha256_hex,
        size_bytes: bytes.len(),
    })
}

fn extract_skill_md(bundle: &Bytes) -> Result<String, BundleError> {
    let gz = GzDecoder::new(bundle.as_ref());
    let mut tar = tar::Archive::new(gz);

    let entries = tar
        .entries()
        .map_err(|e| BundleError::NotAnArchive(e.to_string()))?;

    for entry in entries {
        let mut entry = entry.map_err(|e| BundleError::NotAnArchive(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| BundleError::NotAnArchive(e.to_string()))?
            .to_path_buf();

        let path_str = path.to_string_lossy();

        // Match SKILL.md at the archive root. Tar entries may have a leading
        // `./` so normalise.
        let normalised = path_str.trim_start_matches("./");

        if normalised == "SKILL.md" {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| BundleError::NotAnArchive(e.to_string()))?;
            return Ok(buf);
        }
    }
    Err(BundleError::MissingSkillMd)
}

fn split_frontmatter(content: &str) -> Result<(&str, &str), BundleError> {
    let rest = content
        .strip_prefix("---\n")
        .or_else(|| content.strip_prefix("---\r\n"))
        .ok_or(BundleError::MissingFrontmatter)?;

    // Find the closing `---` on its own line.
    let end_marker_pos = rest
        .find("\n---\n")
        .or_else(|| rest.find("\n---\r\n"))
        .ok_or(BundleError::MissingFrontmatter)?;

    let yaml = &rest[..end_marker_pos];
    let after = &rest[end_marker_pos + 1..]; // skip the leading newline
    let body = after
        .strip_prefix("---\n")
        .or_else(|| after.strip_prefix("---\r\n"))
        .unwrap_or(after);

    Ok((yaml, body))
}

fn check_absolute_paths(body: &str) -> Result<(), BundleError> {
    // Catch the obvious foot-guns. A skill author's home path leaks identity.
    for pat in ["/home/", "/Users/", r"C:\Users\"] {
        if let Some(idx) = body.find(pat) {
            // Slice 32 chars around the hit for the error message.
            let start = idx.saturating_sub(8);
            let end = (idx + 32).min(body.len());
            return Err(BundleError::AbsolutePath(body[start..end].to_string()));
        }
    }
    Ok(())
}

fn check_secrets(content: &str) -> Result<(), BundleError> {
    use regex::Regex;
    // Compile once per call — Phase 5 will cache these via OnceLock.
    let patterns: [(&str, Regex); 4] = [
        (
            "AWS access key ID",
            Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
        ),
        (
            "GitHub personal access token",
            Regex::new(r"\bghp_[A-Za-z0-9]{36}\b").unwrap(),
        ),
        (
            "GitHub OAuth token",
            Regex::new(r"\bgho_[A-Za-z0-9]{36}\b").unwrap(),
        ),
        (
            "PEM private key block",
            Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
        ),
    ];
    for (label, re) in &patterns {
        if re.is_match(content) {
            return Err(BundleError::Secret(label));
        }
    }
    Ok(())
}

/// Build a single-file tar.gz containing a `SKILL.md`. Test helper.
#[cfg(test)]
pub fn make_test_bundle(skill_md: &str) -> Bytes {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let mut tar = tar::Builder::new(Vec::new());
    let bytes = skill_md.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_path("SKILL.md").unwrap();
    header.set_size(bytes.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, bytes).unwrap();
    let tar_bytes = tar.into_inner().unwrap();

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    Bytes::from(gz.finish().unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = "---\nname: foo\ndescription: A test skill that does X reliably.\ntags: [test]\n---\n\n# Foo\n";

    #[test]
    fn happy_path() {
        let bundle = make_test_bundle(VALID);
        let v = validate(&bundle).expect("valid");
        assert_eq!(v.frontmatter.name.as_deref(), Some("foo"));
        assert_eq!(v.frontmatter.tags, vec!["test"]);
        assert_eq!(v.size_bytes, bundle.len());
        assert_eq!(v.sha256_hex.len(), 64);
    }

    #[test]
    fn rejects_missing_frontmatter() {
        let bundle = make_test_bundle("no frontmatter at all\n");
        let err = validate(&bundle).unwrap_err();
        assert!(matches!(err, BundleError::MissingFrontmatter));
    }

    #[test]
    fn rejects_empty_description() {
        let bundle = make_test_bundle("---\ndescription: ''\n---\nbody\n");
        let err = validate(&bundle).unwrap_err();
        assert!(matches!(err, BundleError::BadDescriptionLength(0)));
    }

    #[test]
    fn rejects_overlong_description() {
        let long = "x".repeat(MAX_DESCRIPTION_LEN + 1);
        let bundle = make_test_bundle(&format!("---\ndescription: '{long}'\n---\nbody\n"));
        let err = validate(&bundle).unwrap_err();
        assert!(matches!(err, BundleError::BadDescriptionLength(_)));
    }

    #[test]
    fn rejects_absolute_path() {
        let bundle = make_test_bundle(
            "---\ndescription: short\n---\n\nrun /home/alice/scripts/leak.sh first.\n",
        );
        let err = validate(&bundle).unwrap_err();
        assert!(matches!(err, BundleError::AbsolutePath(_)), "got {err:?}");
    }

    #[test]
    fn rejects_aws_key() {
        let bundle = make_test_bundle(
            "---\ndescription: short\n---\n\ndo not commit AKIAIOSFODNN7EXAMPLE\n",
        );
        let err = validate(&bundle).unwrap_err();
        assert!(matches!(err, BundleError::Secret(_)), "got {err:?}");
    }

    #[test]
    fn rejects_too_large() {
        // Build a bundle whose decompressed size is fine but whose compressed
        // size exceeds the limit. Easiest: skip the gzip and just check the
        // pre-validation size check against arbitrary bytes.
        let bytes = Bytes::from(vec![0u8; MAX_BUNDLE_BYTES + 1]);
        let err = validate(&bytes).unwrap_err();
        assert!(matches!(err, BundleError::TooLarge));
    }
}
