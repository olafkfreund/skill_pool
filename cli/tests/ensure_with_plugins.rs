//! `skill-pool ensure` plugin-resolution integration tests (#36).
//!
//! Three coverage areas — all driven against wiremock so we don't need a
//! live server:
//!
//!   1. **Happy path** — a manifest that pins a plugin walks the plugin's
//!      `contents[]` and installs each bundled atom under the matching
//!      `~/.claude/skills/...` / `agents/...` directory.
//!   2. **Cycle detection** — plugin A → manifest.dependencies=[B],
//!      plugin B → manifest.dependencies=[A]. `ensure` exits 3 (the
//!      dedicated `EXIT_PLUGIN_CYCLE`) and prints a self-contained
//!      diagnostic. `dependencies[]` is the Claude Code plugin.json
//!      field for plugin-to-plugin transitivity (see
//!      `docs/plugin-manifest-schema.md`).
//!   3. **Forward-reference forgiveness** — a plugin slug that 404s
//!      doesn't abort the run; the direct manifest atoms still install.
//!
//! The companion dedup test lives in `ensure_plugin_dedup.rs` so the
//! two stories stay one-test-per-binary at the cargo-test level.

mod common;

use std::io::Write;
use std::path::Path;

use flate2::write::GzEncoder;
use flate2::Compression;
use predicates::str::contains;
use wiremock::matchers::{method, path as wpath, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{skill_pool, write_config};

/// Build a minimal `<kind>.tar.gz` bundle containing one file. The CLI's
/// `install::extract_bundle` accepts any `.tar.gz` layout, so the simplest
/// fixture is a top-level `SKILL.md` (skills/agents/commands all just
/// extract verbatim into a per-slug directory).
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

/// Write a `.skill-pool/manifest.toml` with `[[plugins]]` blocks. No
/// direct `[[skills]]` so the test exercises the plugin-only path.
fn write_plugin_manifest(project_root: &Path, plugin_slugs: &[&str]) {
    let mf_dir = project_root.join(".skill-pool");
    std::fs::create_dir_all(&mf_dir).unwrap();
    let mut body = String::from("[project]\nstack = []\n\n");
    for slug in plugin_slugs {
        body.push_str(&format!(
            "[[plugins]]\nslug = \"{slug}\"\nversion = \"*\"\nscope = \"project\"\n\n"
        ));
    }
    std::fs::write(mf_dir.join("manifest.toml"), body).unwrap();
}

#[tokio::test]
async fn ensure_walks_plugin_contents_and_installs_bundled_atoms() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("workspace");
    std::fs::create_dir_all(&project_root).unwrap();
    write_plugin_manifest(&project_root, &["bundle-alpha"]);

    let server = MockServer::start().await;

    // `GET /v1/plugins/bundle-alpha` returns the plugin detail with two
    // bundled contents: one skill, one agent. Loose manifest passthrough
    // with no nested `plugins[]` field — purely an "atomic bundle" plugin.
    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/bundle-alpha"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "bundle-alpha",
            "version": "1.0.0",
            "name": "Bundle Alpha",
            "description": "Two-item starter bundle",
            "status": "published",
            "sourcing_mode": "internal",
            "manifest": {
                "name": "bundle-alpha",
                "version": "1.0.0"
            },
            "contents": [
                {"kind": "skill", "slug": "a", "version": "1.0", "position": 0},
                {"kind": "agent", "slug": "reviewer", "version": "1.0", "position": 1}
            ],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    // Skill metadata lookup (used by `resolve_version` for the `*`
    // version on the bundled skill — bundled atoms inherit "*" because
    // the resolver leans on the registry to pick the latest).
    Mock::given(method("GET"))
        .and(wpath("/v1/skills/a"))
        .and(query_param("kind", "skill"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "a",
            "version": "1.0",
            "description": "Skill A"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wpath("/v1/skills/reviewer"))
        .and(query_param("kind", "agent"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "reviewer",
            "version": "1.0",
            "description": "Reviewer agent"
        })))
        .mount(&server)
        .await;

    // Bundle downloads — `download_bundle_with_kind` calls
    // `GET /v1/skills/<slug>/bundle.tar.gz?kind=<kind>`. We serve
    // distinct payloads so the on-disk assertion below can verify
    // the right bytes landed at the right path.
    Mock::given(method("GET"))
        .and(wpath("/v1/skills/a/bundle.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(build_bundle_with_marker("MARKER-skill-A"))
                .insert_header("content-type", "application/gzip"),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wpath("/v1/skills/reviewer/bundle.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(build_bundle_with_marker("MARKER-agent-reviewer"))
                .insert_header("content-type", "application/gzip"),
        )
        .mount(&server)
        .await;

    // Catch-all 200 for telemetry POSTs so `--no-telemetry` isn't required.
    Mock::given(method("POST"))
        .and(wpath("/v1/usage"))
        .respond_with(ResponseTemplate::new(204))
        .mount(&server)
        .await;

    let cfg_path = tmp.path().join("config.toml");
    write_config(&cfg_path, &server.uri(), "acme");

    // Point HOME at the tempdir so the install loop writes its `~/.skill-pool`
    // library and `.claude/skills/...` symlinks under our scratch space
    // instead of the real user home directory.
    skill_pool(&project_root, &cfg_path)
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("xdg-data"))
        .args(["ensure", "--no-telemetry"])
        .assert()
        .success()
        .stdout(contains("link:     a"))
        .stdout(contains("link:     reviewer"));

    // The CLI's install layer uses `~/.skill-pool/library/<tenant>/<slug>@<ver>/`
    // and links into `<project>/.claude/skills/<slug>`. (Agents and
    // commands share the same scope-derived target path today — see
    // `install::target_for_scope`; the per-kind directory split is a
    // future refactor, not load-bearing for #36.) We assert the symlink
    // targets exist + their SKILL.md content matches the served bundle
    // bytes — that's the load-bearing on-disk evidence.
    let skill_link = project_root.join(".claude/skills/a/SKILL.md");
    let agent_link = project_root.join(".claude/skills/reviewer/SKILL.md");
    assert!(
        skill_link.exists(),
        "expected bundled skill at {}",
        skill_link.display()
    );
    assert!(
        agent_link.exists(),
        "expected bundled agent at {}",
        agent_link.display()
    );
    let skill_body = std::fs::read_to_string(&skill_link).unwrap();
    let agent_body = std::fs::read_to_string(&agent_link).unwrap();
    assert_eq!(skill_body, "MARKER-skill-A");
    assert_eq!(agent_body, "MARKER-agent-reviewer");
}

#[tokio::test]
async fn ensure_detects_plugin_cycle_and_exits_3() {
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("workspace");
    std::fs::create_dir_all(&project_root).unwrap();
    write_plugin_manifest(&project_root, &["a"]);

    let server = MockServer::start().await;

    // Plugin A's manifest declares it requires plugin B via the
    // Claude Code plugin.json `dependencies[]` spec field. We use the
    // bare-string shorthand here (equivalent to `{name:"b",version:"*"}`)
    // and the full object shape in B below, so this single test exercises
    // both forms of the spec-allowed entry shape.
    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/a"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "a",
            "version": "1.0.0",
            "name": "A",
            "description": "A",
            "status": "published",
            "sourcing_mode": "internal",
            "manifest": {
                "name": "a",
                "version": "1.0.0",
                "dependencies": ["b"]
            },
            "contents": [],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    // Plugin B closes the loop with the object shape (`{name, version}`).
    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/b"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "b",
            "version": "1.0.0",
            "name": "B",
            "description": "B",
            "status": "published",
            "sourcing_mode": "internal",
            "manifest": {
                "name": "b",
                "version": "1.0.0",
                "dependencies": [{"name": "a", "version": "^1.0.0"}]
            },
            "contents": [],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;

    let cfg_path = tmp.path().join("config.toml");
    write_config(&cfg_path, &server.uri(), "acme");

    skill_pool(&project_root, &cfg_path)
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("xdg-data"))
        .args(["ensure", "--no-telemetry"])
        .assert()
        // EXIT_PLUGIN_CYCLE = 3. The dedicated code lets CI scripts
        // distinguish a manifest correctness error from a generic
        // anyhow-exit-1 ("registry is down") without grepping stderr.
        .code(3)
        // The diagnostic is normalised so the smallest slug leads
        // (a < b → "a → b → a"), and the suggested fix is named.
        .stderr(contains("plugin dependency cycle detected"))
        .stderr(contains("a → b → a"))
        .stderr(contains("remove the back-reference"));
}

#[tokio::test]
async fn ensure_continues_when_a_plugin_is_unpublished() {
    // Mix one resolvable plugin with one 404'd plugin. The 404 path is
    // already exercised by `ensure_walks_plugin_contents_and_installs_bundled_atoms`
    // when wiremock has zero stubs for an endpoint, but pairing them
    // here proves the warn-and-continue branch leaves the rest of the
    // install plan intact.
    let tmp = tempfile::tempdir().unwrap();
    let project_root = tmp.path().join("workspace");
    std::fs::create_dir_all(&project_root).unwrap();
    write_plugin_manifest(&project_root, &["bundle-alpha", "ghost"]);

    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/bundle-alpha"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "bundle-alpha",
            "version": "1.0.0",
            "name": "Bundle Alpha",
            "description": "Single-item",
            "status": "published",
            "sourcing_mode": "internal",
            "manifest": {"name": "bundle-alpha", "version": "1.0.0"},
            "contents": [
                {"kind": "skill", "slug": "a", "version": "1.0", "position": 0}
            ],
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wpath("/v1/plugins/ghost"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(wpath("/v1/skills/a"))
        .and(query_param("kind", "skill"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "slug": "a",
            "version": "1.0",
            "description": "Skill A"
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(wpath("/v1/skills/a/bundle.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(build_bundle_with_marker("MARKER-A"))
                .insert_header("content-type", "application/gzip"),
        )
        .mount(&server)
        .await;

    let cfg_path = tmp.path().join("config.toml");
    write_config(&cfg_path, &server.uri(), "acme");

    skill_pool(&project_root, &cfg_path)
        .env("HOME", tmp.path())
        .env("XDG_DATA_HOME", tmp.path().join("xdg-data"))
        .args(["ensure", "--no-telemetry"])
        .assert()
        .success()
        // The resolvable plugin's content lands ...
        .stdout(contains("link:     a"))
        // ... and the unresolvable plugin warns rather than aborting.
        .stdout(contains("plugin `ghost` not found"));

    assert!(project_root.join(".claude/skills/a/SKILL.md").exists());
}
