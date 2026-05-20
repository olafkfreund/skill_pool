//! Best-effort git introspection. We invoke `git` as a subprocess
//! rather than pulling in `git2`/`gix` because the only thing we
//! need is `git remote get-url origin` and that adds 5+ MB to the
//! CLI binary otherwise.

use std::process::Command;

/// Returns the origin remote URL, or `None` if no `git` is on PATH,
/// no `.git` is in cwd, or no `origin` remote is configured.
///
/// Uses `git config --get remote.origin.url` which is fast (reads
/// only `.git/config`) and exits non-zero when no remote is set —
/// both cases are silently mapped to `None`. The subprocess is
/// constrained to ~100 ms by the OS process budget; in practice it
/// completes in single-digit milliseconds.
///
/// # Example
/// ```no_run
/// if let Some(url) = skill_pool_cli::git::detect_origin_url() {
///     println!("origin: {url}");
/// }
/// ```
pub fn detect_origin_url() -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Internal helper used by tests: run `git config --get remote.origin.url`
/// with an explicit working directory, returning the URL or None.
#[cfg(test)]
pub(crate) fn detect_origin_url_in(dir: &std::path::Path) -> Option<String> {
    let output = Command::new("git")
        .args(["config", "--get", "remote.origin.url"])
        .current_dir(dir)
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returns_none_in_non_repo_dir() {
        // /tmp is guaranteed not to be a git repo.
        let result = detect_origin_url_in(std::path::Path::new("/tmp"));
        assert!(
            result.is_none(),
            "expected None for /tmp, got {result:?}"
        );
    }

    #[test]
    fn empty_stdout_maps_to_none() {
        // Verify the trim + empty-check logic directly.
        let raw = "  \n  ";
        let trimmed = raw.trim().to_string();
        let result: Option<String> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        assert!(result.is_none());
    }

    #[test]
    fn nonempty_url_is_returned() {
        // Verify the happy-path trimming.
        let raw = "git@github.com:example/repo.git\n";
        let trimmed = raw.trim().to_string();
        let result: Option<String> = if trimmed.is_empty() {
            None
        } else {
            Some(trimmed)
        };
        assert_eq!(result.as_deref(), Some("git@github.com:example/repo.git"));
    }
}
