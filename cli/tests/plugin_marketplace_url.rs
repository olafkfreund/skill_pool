//! `skill-pool plugin marketplace-url` integration test (#33).
//!
//! Verifies the printed URL matches the documented marketplace shape:
//! `https://<tenant>.<registry-host>/.claude-plugin/marketplace.json`
//! (per `docs/tenancy.md:39`).

mod common;

use predicates::str::contains;

use crate::common::{skill_pool, write_config};

#[test]
fn marketplace_url_prints_canonical_tenant_subdomain_url() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "https://registry.example.com", "acme");

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "marketplace-url"])
        .assert()
        .success()
        .stdout(contains(
            "https://acme.registry.example.com/.claude-plugin/marketplace.json",
        ));
}

#[test]
fn marketplace_url_does_not_double_prefix_tenanted_registry() {
    // If the operator already configured a tenant-subdomain URL, we
    // must not produce `acme.acme.registry…`.
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "https://acme.registry.example.com", "acme");

    let assert = skill_pool(tmp.path(), &cfg)
        .args(["plugin", "marketplace-url"])
        .assert()
        .success();
    let stdout = String::from_utf8(assert.get_output().stdout.clone()).unwrap();
    assert_eq!(
        stdout.trim(),
        "https://acme.registry.example.com/.claude-plugin/marketplace.json",
        "stdout should be exactly the URL with no double-prefix: {stdout:?}"
    );
}

#[test]
fn marketplace_url_handles_localhost_dev_port() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "http://localhost:8080", "acme");

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "marketplace-url"])
        .assert()
        .success()
        .stdout(contains(
            "http://acme.localhost:8080/.claude-plugin/marketplace.json",
        ));
}
