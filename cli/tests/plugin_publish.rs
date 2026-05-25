//! `skill-pool plugin publish <dir>` integration test (#33).
//!
//! Server-side `POST /v1/plugins` is in flight (#30). Two cases:
//!   1. **Happy path**: wiremock returns 201 → CLI prints "published …"
//!      and exits 0.
//!   2. **Server not yet shipped**: wiremock returns 404 → CLI falls
//!      back with the "tracking: issue #30" message and exits **2**
//!      (operation unavailable). Exit 0 would silently break shell
//!      chains like `publish && deploy` because the user asked us to
//!      publish, not to validate.

mod common;

use std::path::Path;

use predicates::str::contains;
use wiremock::matchers::{body_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::common::{skill_pool, write_config};

/// Write a minimal valid `.claude-plugin/plugin.json` under `plugin_dir`.
fn write_plugin_json(plugin_dir: &Path, name: &str, version: &str) {
    let claude_dir = plugin_dir.join(".claude-plugin");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let body = serde_json::json!({
        "name": name,
        "version": version,
        "description": "Test plugin fixture",
        "contents": []
    });
    std::fs::write(
        claude_dir.join("plugin.json"),
        serde_json::to_vec_pretty(&body).unwrap(),
    )
    .unwrap();
}

#[tokio::test]
async fn plugin_publish_happy_path_against_201_server() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join("my-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    write_plugin_json(&plugin_dir, "my-plugin", "1.0.0");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/plugins"))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "slug": "my-plugin",
            "version": "1.0.0",
            "status": "published",
        })))
        .mount(&server)
        .await;

    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, &server.uri(), "acme");

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "publish"])
        .arg(&plugin_dir)
        .assert()
        .success()
        .stdout(contains("validated: my-plugin@1.0.0"))
        .stdout(contains("published: my-plugin@1.0.0"));
}

#[tokio::test]
async fn plugin_publish_falls_back_cleanly_when_server_returns_404() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join("my-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    write_plugin_json(&plugin_dir, "my-plugin", "1.0.0");

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/plugins"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, &server.uri(), "acme");

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "publish"])
        .arg(&plugin_dir)
        .assert()
        // Exit 2 so `publish && deploy` chains halt — exit 0 would
        // silently advance past the unpublished plugin.
        .code(2)
        .stdout(contains("validated: my-plugin@1.0.0"))
        .stdout(contains("tracking: issue #30"));
}

/// Wire-shape regression for #57: the CLI must POST a `PublishBody`
/// envelope, not the bare `PluginManifest`. The server side rejects the
/// bare manifest with 400/422; this test pins the contract from the CLI
/// side so the two halves stay in lockstep.
#[tokio::test]
async fn plugin_publish_posts_publishbody_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let plugin_dir = tmp.path().join("my-plugin");
    std::fs::create_dir_all(&plugin_dir).unwrap();

    // Manifest with one bundled skill — exercises the contents passthrough.
    let claude_dir = plugin_dir.join(".claude-plugin");
    std::fs::create_dir_all(&claude_dir).unwrap();
    let manifest = serde_json::json!({
        "name": "my-plugin",
        "version": "1.0.0",
        "description": "Test plugin fixture",
        "contents": [
            { "kind": "skill", "slug": "a11y-audit", "version": "1.0.0" }
        ]
    });
    std::fs::write(
        claude_dir.join("plugin.json"),
        serde_json::to_vec_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let expected_envelope = serde_json::json!({
        "slug": "my-plugin",
        "manifest": {
            "name": "my-plugin",
            "version": "1.0.0",
            "description": "Test plugin fixture",
            "contents": [
                { "kind": "skill", "slug": "a11y-audit", "version": "1.0.0" }
            ]
        },
        "contents": [
            { "kind": "skill", "slug": "a11y-audit", "version": "1.0.0" }
        ],
        "sourcing_mode": "internal",
        "status": "published",
    });

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/plugins"))
        .and(body_json(&expected_envelope))
        .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
            "slug": "my-plugin",
            "version": "1.0.0",
            "status": "published",
        })))
        .mount(&server)
        .await;

    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, &server.uri(), "acme");

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "publish"])
        .arg(&plugin_dir)
        .assert()
        .success()
        .stdout(contains("published: my-plugin@1.0.0"));
}

#[test]
fn plugin_publish_rejects_directory_without_plugin_json() {
    let tmp = tempfile::tempdir().unwrap();
    let empty_dir = tmp.path().join("not-a-plugin");
    std::fs::create_dir_all(&empty_dir).unwrap();

    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "http://localhost:9", "acme");

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "publish"])
        .arg(&empty_dir)
        .assert()
        .failure()
        .stderr(contains(".claude-plugin/plugin.json"));
}
