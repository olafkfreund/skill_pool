//! Best-effort Git mirror of the skill catalog.
//!
//! Per the master plan's two-way sync, every successful publish should
//! eventually land in a Git repo so operators have an audit-grade
//! human-readable history of the catalog. Postgres remains the source
//! of truth — Git is a mirror.
//!
//! Design choices:
//!   * **Optional.** Disabled unless `SKILL_POOL_GIT_REPO_PATH` is set.
//!     A deploy that doesn't want the Git side gets a no-op.
//!   * **Best-effort.** Any error — repo missing, lock contention,
//!     untrusted-dir refusal — logs a warning and returns `Ok(None)`.
//!     The publish response is NEVER blocked on git success.
//!   * **`std::process::Command` over `git2`.** The latter is a 30 MB
//!     C dep with libssh2; the former is what every CI box already
//!     has installed. We don't need lib-grade primitives — we need
//!     `add` and `commit` to land.
//!
//! Layout on disk:
//!
//! ```text
//! <repo>/<tenant_slug>/<kind>/<slug>/<version>/SKILL.md
//!                                              <other-extracted-files>
//! ```
//!
//! Kind is one of `skill`, `agent`, `command`. Each publish writes a
//! single commit with subject `publish: <tenant>/<kind>/<slug>@<version>`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{anyhow, Context, Result};
use flate2::read::GzDecoder;
use tokio::process::Command;

/// Run a `git` subcommand. Returns Err if the command fails to spawn or
/// exits non-zero; otherwise returns the captured stdout (trimmed).
async fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("spawn `git {}`", args.join(" ")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "git {} failed ({}): {}",
            args.join(" "),
            output.status,
            stderr.trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Commit a freshly-published skill into the optional Git mirror.
///
/// `skill_md` is the canonical SKILL.md text (the one the server held
/// at publish time). `bundle_bytes` is the raw gzipped tarball — every
/// non-SKILL.md entry in it is extracted alongside the canonical
/// SKILL.md write. Pass `bundle_bytes = &[]` if you only need to
/// commit the SKILL.md (e.g. fresh draft promotion where the bundle is
/// effectively just SKILL.md).
///
/// Returns:
///   * `Ok(None)` — git sync disabled, OR a best-effort failure was
///     swallowed. Publish flow continues unaffected.
///   * `Ok(Some(sha))` — commit landed; the abbreviated SHA is the
///     newly-created `HEAD`.
///   * `Err(_)` — should be rare: argument validation only (empty
///     slugs, invalid path components). Callers wrap this in a `let _`
///     anyway, so even Err is safely ignored.
pub async fn commit_skill(
    repo_path: &Path,
    tenant_slug: &str,
    kind: &str,
    slug: &str,
    version: &str,
    skill_md: &str,
    bundle_bytes: &[u8],
) -> Result<Option<String>> {
    // Argument sanity. Reject anything that would let a hostile slug
    // escape the per-tenant directory — `..`, leading `/`, NUL.
    for (label, value) in [
        ("tenant_slug", tenant_slug),
        ("kind", kind),
        ("slug", slug),
        ("version", version),
    ] {
        if value.is_empty() {
            return Err(anyhow!("git_sync: {label} must not be empty"));
        }
        if value.contains(['/', '\\', '\0']) || value == "." || value == ".." {
            return Err(anyhow!(
                "git_sync: {label} contains forbidden chars: {value}"
            ));
        }
    }

    if !repo_path.exists() {
        tracing::warn!(
            repo = %repo_path.display(),
            "git_sync: repo path does not exist; skipping",
        );
        return Ok(None);
    }

    // Allow upgrades to swap repo ownership without surgery on git's
    // safe.directory list. Best-effort: if it fails, the subsequent
    // git commands will surface the same error and we'll log+return None.
    let _ = git(repo_path, &["rev-parse", "--git-dir"]).await;

    let target_dir = repo_path
        .join(tenant_slug)
        .join(kind)
        .join(slug)
        .join(version);
    if let Err(e) = std::fs::create_dir_all(&target_dir) {
        tracing::warn!(
            error = %e,
            path = %target_dir.display(),
            "git_sync: mkdir failed; skipping",
        );
        return Ok(None);
    }

    // Always write SKILL.md from the canonical source the server holds
    // in memory. Then, if a tarball was supplied, layer its remaining
    // entries (skipping its own SKILL.md so we never overwrite the
    // canonical copy).
    let skill_md_path = target_dir.join("SKILL.md");
    if let Err(e) = std::fs::write(&skill_md_path, skill_md) {
        tracing::warn!(
            error = %e,
            path = %skill_md_path.display(),
            "git_sync: write SKILL.md failed; skipping",
        );
        return Ok(None);
    }

    if !bundle_bytes.is_empty() {
        if let Err(e) = extract_bundle_into(bundle_bytes, &target_dir) {
            tracing::warn!(
                error = %e,
                "git_sync: bundle extraction failed; SKILL.md kept, continuing",
            );
        }
    }

    // The path argument is relative-to-repo because that's what git
    // wants. Use the same forward-slash separator on all platforms.
    let rel_path = format!("{tenant_slug}/{kind}/{slug}/{version}");
    let subject = format!("publish: {tenant_slug}/{kind}/{slug}@{version}");

    match do_commit(repo_path, &rel_path, &subject).await {
        Ok(sha) => Ok(Some(sha)),
        Err(e) => {
            tracing::warn!(
                error = %e,
                tenant = tenant_slug,
                slug,
                version,
                "git_sync: commit failed; publish unaffected",
            );
            Ok(None)
        }
    }
}

async fn do_commit(repo: &Path, rel_path: &str, subject: &str) -> Result<String> {
    git(repo, &["add", "--", rel_path])
        .await
        .context("git add")?;

    // If the working tree didn't actually change (re-publish of the
    // same content), bail out cleanly with the current HEAD so callers
    // can still log the resulting SHA.
    let diff = git(repo, &["diff", "--cached", "--name-only"]).await?;
    if diff.trim().is_empty() {
        let head = git(repo, &["rev-parse", "HEAD"]).await.unwrap_or_default();
        tracing::info!(rel_path, "git_sync: nothing to commit; reusing HEAD");
        return Ok(head);
    }

    git(
        repo,
        &[
            "-c",
            "user.email=skill-pool@local",
            "-c",
            "user.name=skill-pool",
            "commit",
            "-m",
            subject,
        ],
    )
    .await
    .context("git commit")?;

    git(repo, &["rev-parse", "HEAD"])
        .await
        .context("rev-parse HEAD")
}

/// Decompress `bundle_bytes` (gz-tar produced by `bundle::validate`) into
/// `target_dir`. Skips the bundle's own `SKILL.md` so the caller's
/// canonical write wins. Path traversal is rejected.
fn extract_bundle_into(bundle_bytes: &[u8], target_dir: &Path) -> Result<()> {
    let gz = GzDecoder::new(bundle_bytes);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries().context("read tar entries")? {
        let mut entry = entry.context("read tar entry header")?;
        let path = entry.path().context("entry path")?.into_owned();
        let str_path = path.to_string_lossy();
        let normalised = str_path.trim_start_matches("./");

        // Skip the canonical SKILL.md — caller already wrote it.
        if normalised == "SKILL.md" {
            continue;
        }

        // Forbid traversal and absolute paths.
        let safe_path = sanitize_entry_path(normalised)?;
        let dest = target_dir.join(safe_path);

        // Directories.
        let header = entry.header().clone();
        if header.entry_type().is_dir() {
            std::fs::create_dir_all(&dest).with_context(|| format!("mkdir {}", dest.display()))?;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("mkdir {}", parent.display()))?;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .with_context(|| format!("read tar body of {}", normalised))?;
        std::fs::write(&dest, &buf).with_context(|| format!("write {}", dest.display()))?;
    }
    Ok(())
}

fn sanitize_entry_path(raw: &str) -> Result<PathBuf> {
    let p = Path::new(raw);
    if p.is_absolute() {
        return Err(anyhow!("absolute path in bundle: {raw}"));
    }
    for component in p.components() {
        match component {
            std::path::Component::ParentDir | std::path::Component::RootDir => {
                return Err(anyhow!("path traversal in bundle: {raw}"));
            }
            _ => {}
        }
    }
    Ok(p.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as SyncCommand;
    use tempfile::TempDir;

    /// Bootstrap a tiny git repo via the actual binary. Tests skip
    /// themselves if `git` isn't on PATH so the suite stays green on
    /// containers that don't ship it.
    fn init_repo() -> Option<TempDir> {
        let tmp = TempDir::new().ok()?;
        let init = SyncCommand::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["-c", "init.defaultBranch=main", "init", "-q"])
            .status()
            .ok()?;
        if !init.success() {
            return None;
        }
        // Configure an identity locally so commits don't fail on a
        // user-less CI box.
        let _ = SyncCommand::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["config", "user.email", "test@local"])
            .status();
        let _ = SyncCommand::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["config", "user.name", "test"])
            .status();
        // An empty `commit --allow-empty` so `HEAD` resolves before our
        // first real commit, exercising the same code paths in prod.
        let _ = SyncCommand::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["commit", "--allow-empty", "-m", "root", "-q"])
            .status();
        Some(tmp)
    }

    fn git_available() -> bool {
        SyncCommand::new("git")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn commit_skill_writes_file_and_returns_sha() {
        if !git_available() {
            eprintln!("git not on PATH; skipping");
            return;
        }
        let Some(tmp) = init_repo() else {
            eprintln!("failed to bootstrap repo; skipping");
            return;
        };

        let sha = commit_skill(
            tmp.path(),
            "acme",
            "skill",
            "axum-tip",
            "1.0.0",
            "---\nname: axum-tip\ndescription: x\n---\n\nbody\n",
            &[],
        )
        .await
        .expect("commit_skill should not Err on happy path");
        let sha = sha.expect("Some(sha) on success");
        assert_eq!(sha.len(), 40, "full sha");

        // File landed at the expected path.
        let landed = tmp.path().join("acme/skill/axum-tip/1.0.0/SKILL.md");
        assert!(landed.exists(), "expected file at {}", landed.display());

        // git log -1 --pretty=%s shows our subject.
        let out = SyncCommand::new("git")
            .arg("-C")
            .arg(tmp.path())
            .args(["log", "-1", "--pretty=%s"])
            .output()
            .expect("git log");
        let subject = String::from_utf8_lossy(&out.stdout);
        assert!(
            subject.contains("publish: acme/skill/axum-tip@1.0.0"),
            "subject was: {subject}"
        );
    }

    #[tokio::test]
    async fn commit_skill_returns_none_when_repo_path_missing() {
        // Best-effort policy: a missing repo logs a warning but never errs.
        let res = commit_skill(
            Path::new("/nonexistent/skill-pool-mirror"),
            "acme",
            "skill",
            "slug",
            "1.0.0",
            "---\ndescription: x\n---\n",
            &[],
        )
        .await
        .expect("never Errs on missing repo");
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn commit_skill_rejects_path_traversal_in_args() {
        let Some(tmp) = init_repo() else {
            eprintln!("git not on PATH; skipping");
            return;
        };
        let res = commit_skill(
            tmp.path(),
            "acme",
            "skill",
            "../escape",
            "1.0.0",
            "---\ndescription: x\n---\n",
            &[],
        )
        .await;
        assert!(res.is_err(), "traversal in slug must Err");
    }

    #[tokio::test]
    async fn commit_skill_extracts_bundle_contents() {
        if !git_available() {
            eprintln!("git not on PATH; skipping");
            return;
        }
        let Some(tmp) = init_repo() else {
            eprintln!("failed to bootstrap repo; skipping");
            return;
        };

        // Hand-roll a tiny gz-tar with a single extra file.
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;
        let mut tar = tar::Builder::new(Vec::new());
        let body = b"hello extra\n";
        let mut hdr = tar::Header::new_gnu();
        hdr.set_path("examples/quick.md").unwrap();
        hdr.set_size(body.len() as u64);
        hdr.set_mode(0o644);
        hdr.set_cksum();
        tar.append(&hdr, &body[..]).unwrap();
        let tar_bytes = tar.into_inner().unwrap();
        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        let bundle = gz.finish().unwrap();

        let res = commit_skill(
            tmp.path(),
            "acme",
            "skill",
            "withbundle",
            "1.0.0",
            "---\ndescription: x\n---\nbody\n",
            &bundle,
        )
        .await
        .expect("ok");
        assert!(res.is_some());
        let extra = tmp
            .path()
            .join("acme/skill/withbundle/1.0.0/examples/quick.md");
        assert!(
            extra.exists(),
            "extracted file should exist at {}",
            extra.display()
        );
        let content = std::fs::read_to_string(&extra).unwrap();
        assert_eq!(content, "hello extra\n");
    }

    #[test]
    fn sanitize_entry_path_rejects_traversal() {
        assert!(sanitize_entry_path("../etc/passwd").is_err());
        assert!(sanitize_entry_path("/etc/passwd").is_err());
        assert!(sanitize_entry_path("ok/relative.md").is_ok());
    }
}
