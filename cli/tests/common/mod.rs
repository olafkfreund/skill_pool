//! Shared fixtures for the `skill-pool plugin` integration tests (#33).
//!
//! Each test creates a fresh tempdir, optionally seeds a manifest +
//! a stub config, then invokes the compiled `skill-pool` binary against
//! that tempdir as `--config`. No global state, no shared fixtures —
//! tests can run in parallel safely.

use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// Build a `Command` for the `skill-pool` binary, with the working
/// directory set to `cwd` and `--config` pointed at `config_path`.
///
/// `SKILL_POOL_NO_BANNER=1` is set unconditionally so the banner fetch
/// in `banner::show_if_due` doesn't try to hit the network during tests.
pub fn skill_pool(cwd: &Path, config_path: &Path) -> Command {
    let mut cmd = Command::cargo_bin("skill-pool").expect("compile skill-pool binary");
    cmd.current_dir(cwd)
        .env("SKILL_POOL_NO_BANNER", "1")
        .arg("--config")
        .arg(config_path);
    cmd
}

/// Write a config file pinned to the given registry URL + tenant. No token —
/// the plugin subcommands don't require one for the local-only path
/// (`add`, `marketplace-url`) and the network-touching tests use
/// wiremock which doesn't enforce auth.
pub fn write_config(path: &Path, registry_url: &str, tenant: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create config dir");
    }
    let raw = format!("[registry]\nurl = \"{registry_url}\"\ntenant = \"{tenant}\"\n",);
    std::fs::write(path, raw).expect("write config");
}

/// Seed a starter `.skill-pool/manifest.toml` under `project_root` with
/// a single skill entry so the file isn't entirely empty. Mirrors what
/// `skill-pool init` would produce; kept here so tests don't have to
/// shell out to a second subcommand to set up state.
///
/// `#[allow(dead_code)]`: `common/mod.rs` is compiled once per test
/// binary, but only `plugin_add.rs` uses this helper — the other two
/// binaries (`plugin_publish`, `plugin_marketplace_url`) flag it
/// otherwise. Standard cargo-test layout caveat.
#[allow(dead_code)]
pub fn write_starter_manifest(project_root: &Path) -> PathBuf {
    let path = project_root.join(".skill-pool").join("manifest.toml");
    std::fs::create_dir_all(path.parent().unwrap()).expect("create manifest dir");
    let raw = r#"[project]
stack = ["rust"]

[[skills]]
slug = "code-review-mastery"
version = "*"
scope = "project"
"#;
    std::fs::write(&path, raw).expect("write manifest");
    path
}
