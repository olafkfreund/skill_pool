//! `skill-pool plugin add <spec>` integration test (#33).
//!
//! Verifies the three documented branches of `add_plugin_to_manifest`:
//!   1. Insert a new `[[plugins]]` block.
//!   2. Re-add the same `(slug, version)` → no-op, byte-identical TOML.
//!   3. Re-add a different version → update-in-place.

mod common;

use predicates::str::contains;

use crate::common::{skill_pool, write_config, write_starter_manifest};

/// Manifest the tests start from — exists in the parent project root via
/// `write_starter_manifest`. `plugin add` walks up from the cwd to find
/// the `.skill-pool/manifest.toml`, so cwd = tempdir root is enough.
#[test]
fn plugin_add_inserts_block_and_round_trips() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "http://localhost:9", "acme"); // unused — add is pure-local
    let manifest_path = write_starter_manifest(tmp.path());

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "add", "acme-toolkit@1.2.0"])
        .assert()
        .success()
        .stdout(contains("added: acme-toolkit@1.2.0"));

    let after = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        after.contains("[[plugins]]"),
        "manifest should now contain a [[plugins]] block:\n{after}"
    );
    assert!(after.contains("slug = \"acme-toolkit\""));
    assert!(after.contains("version = \"1.2.0\""));

    // Round-trip stability: invoke `plugin add` again with the same spec
    // (a documented no-op) and assert the file is byte-identical.
    // Catches accidental writer drift across saves of the same logical
    // manifest content. (We can't round-trip via `toml::Value` here —
    // it's a sorted map, which would reorder keys; the typed `Manifest`
    // path is what `save_in` uses and what we care about.)
    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "add", "acme-toolkit@1.2.0"])
        .assert()
        .success();
    let after_second = std::fs::read_to_string(&manifest_path).unwrap();
    assert_eq!(
        after, after_second,
        "second no-op `plugin add` must leave the manifest byte-identical"
    );
}

#[test]
fn plugin_add_is_noop_for_same_version() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "http://localhost:9", "acme");
    let manifest_path = write_starter_manifest(tmp.path());

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "add", "acme-toolkit@1.2.0"])
        .assert()
        .success();
    let after_first = std::fs::read_to_string(&manifest_path).unwrap();

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "add", "acme-toolkit@1.2.0"])
        .assert()
        .success()
        .stdout(contains("(already in manifest: acme-toolkit@1.2.0)"));
    let after_second = std::fs::read_to_string(&manifest_path).unwrap();

    assert_eq!(
        after_first, after_second,
        "second `plugin add` of the same spec must be a byte-identical no-op"
    );
}

#[test]
fn plugin_add_updates_in_place_on_version_change() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");
    write_config(&cfg, "http://localhost:9", "acme");
    let manifest_path = write_starter_manifest(tmp.path());

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "add", "acme-toolkit@1.2.0"])
        .assert()
        .success();

    skill_pool(tmp.path(), &cfg)
        .args(["plugin", "add", "acme-toolkit@1.3.0"])
        .assert()
        .success()
        .stdout(contains("updated: acme-toolkit 1.2.0 → 1.3.0"));

    let after = std::fs::read_to_string(&manifest_path).unwrap();
    // Exactly one [[plugins]] block survives.
    let block_count = after.matches("[[plugins]]").count();
    assert_eq!(
        block_count, 1,
        "version update must not append a second block:\n{after}"
    );
    assert!(after.contains("version = \"1.3.0\""));
    assert!(!after.contains("version = \"1.2.0\""));
}
