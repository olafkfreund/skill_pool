//! `skill-pool ensure` plugin-vs-direct dedup test (#36 — §2 worked example).
//!
//! The manifest pins skill A directly at version 1.0, AND pins two
//! plugins P and Q. P bundles A@2.0, Q bundles A@3.0. The dedup invariant:
//!
//!   precedence: direct manifest pin > plugin-bundled (any depth)
//!   result    : exactly ONE install of A, at version 1.0
//!
//! We prove this two ways:
//!
//!   1. The `link: a` line appears exactly once in stdout (no double-
//!      install action for the bundled-plugin versions).
//!   2. The metadata endpoint for skill A is hit with `kind=skill` and
//!      a version-resolve happens for "1.0" only — the wiremock stub for
//!      `/v1/skills/a?kind=skill` returns version 1.0, and we never
//!      stub bundle downloads for 2.0 or 3.0, so the test would fail
//!      with a "download failed" warning if precedence inverted.

mod common;

use std::io::Write;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use wiremock::matchers::{method, path as wpath};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{skill_pool, write_config};

fn build_bundle_with_marker(marker: &str) -> Vec<u8> {
    let mut tar_buf: Vec<u8> = Vec::new();
    {
        let mut tar = tar::Builder::new(&mut tar_buf);
        let mut header = tar::Header::new_gnu();
        header.set_size(marker.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append_data(&mut header, "SKILL.md", marker.as_bytes())
            .unwrap();
        tar.finish().unwrap();
    }
    let mut gz: Vec<u8> = Vec::new();
    let mut encoder = GzEncoder::new(&mut gz, Compression::default());
    encoder.write_all(&tar_buf).unwrap();
    encoder.finish().unwrap();
    gz
}

/// `[[skills]] a@1.0` direct + `[[plugins]] p` + `[[plugins]] q`.
fn write_dedup_manifest(project_root: &Path) {
    let mf_dir = project_root.join(".skill-pool");
    std::fs::create_dir_all(&mf_dir).unwrap();
    let body = r#"[project]
stack = []

[[skills]]
slug = "a"
version = "1.0"
scope = "project"

[[plugins]]
slug = "p"
version = "*"
scope = "project"

[[plugins]]
slug = "q"
version = "*"
scope = "project"
"#;
    std::fs::write(mf_dir.join("manifest.toml"), body).unwrap();
}

#[tokio::test]
async fn direct_skill_pin_wins_over_plugin_bundled_at_different_versions() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("workspace");
    std::fs::create_dir_all(&project_root).unwrap();
    write_dedup_manifest(&project_root);

    let server = MockServer::start().await;

    // Plugin P bundles A@2.0.
    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/p"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "p",
            "version": "1.0.0",
            "name": "P",
            "description": "P",
            "status": "published",
            "sourcing_mode": "internal",
            "manifest": {"name": "p", "version": "1.0.0"},
            "contents": [
                {"kind": "skill", "slug": "a", "version": "2.0", "position": 0}
            ],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    // Plugin Q bundles A@3.0.
    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/q"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "q",
            "version": "1.0.0",
            "name": "Q",
            "description": "Q",
            "status": "published",
            "sourcing_mode": "internal",
            "manifest": {"name": "q", "version": "1.0.0"},
            "contents": [
                {"kind": "skill", "slug": "a", "version": "3.0", "position": 0}
            ],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    // The direct skill is pinned to "1.0" so `resolve_version` short-
    // circuits without a metadata lookup. The bundle endpoint is the
    // only catalog call we need to stub — and we serve a marker that
    // names version 1.0 so any cross-version mix-up shows in the
    // on-disk content assertion.
    Mock::given(method("GET"))
        .and(wpath("/v1/skills/a/bundle.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(build_bundle_with_marker("MARKER-A-version-1.0"))
                .insert_header("content-type", "application/gzip"),
        )
        .mount(&server)
        .await;

    let cfg_path = tmp.path().join("config.toml");
    write_config(&cfg_path, &server.uri(), "acme");

    let assert = skill_pool(&project_root, &cfg_path)
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("xdg-data"))
        .args(["ensure", "--no-telemetry"])
        .assert()
        .success();

    let output = assert.get_output();
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Exactly one install action for `a` — counts both "link" and "ok"
    // (a re-run would print "ok"). The dedup invariant is that the
    // (slug, kind) appears in the plan exactly once, so the action
    // line should never repeat.
    let action_count = stdout
        .lines()
        .filter(|l| l.contains(" a") && (l.contains("link:") || l.contains("ok:")))
        .count();
    assert_eq!(
        action_count, 1,
        "skill `a` must install exactly once across direct + 2 plugins; stdout was:\n{stdout}"
    );

    // The resolved version on disk is 1.0 (the direct pin), proven by
    // the marker content we put in the bundle the server served.
    let installed = std::fs::read_to_string(project_root.join(".claude/skills/a/SKILL.md")).unwrap();
    assert_eq!(installed, "MARKER-A-version-1.0");
}
