//! Issue #32 — plugin mirror: clone + manifest parse + marketplace update.
//!
//! Flow:
//!   1. Boot Postgres + migrations.
//!   2. Create an upstream bare repo in a tempdir containing a valid
//!      `.claude-plugin/plugin.json` manifest, committed on `main`.
//!   3. Insert a mirror plugin stub (simulates what /v1/plugins/import does).
//!   4. Call `run_mirror` directly to simulate the job handler running
//!      synchronously (same pattern as the project_plans tests which call
//!      admin fns directly rather than going through the queue).
//!   5. Assert:
//!        - `plugins` row has `last_pulled_at` NOT NULL, `fetch_error` NULL.
//!        - `plugin_marketplace_entries` row exists.
//!        - The local bare repo at `storage.plugin_git_path(...)` is readable.
//!        - `.claude-plugin/plugin.json` in the local repo matches the upstream.
//!
//! The "in-process git server" uses a bare repo on the local filesystem
//! accessed via `file://` URL — same libgit2 code path as network clones.

use std::path::Path;

use anyhow::Result;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{
    admin,
    jobs::plugin_mirror::{run_mirror, PluginMirrorJob},
    storage::Storage,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_upstream_repo(repo_path: &Path, manifest: &serde_json::Value) -> Result<git2::Oid> {
    use git2::{Repository, Signature};

    let repo = Repository::init_bare(repo_path)?;
    let manifest_bytes = serde_json::to_vec_pretty(manifest)?;
    let blob_oid = repo.blob(&manifest_bytes)?;

    let mut inner = repo.treebuilder(None)?;
    inner.insert("plugin.json", blob_oid, 0o100644)?;
    let inner_oid = inner.write()?;

    let mut root = repo.treebuilder(None)?;
    root.insert(".claude-plugin", inner_oid, 0o040000)?;
    let root_oid = root.write()?;

    let tree = repo.find_tree(root_oid)?;
    let sig = Signature::now("test", "test@example.com")?;
    let commit_oid = repo.commit(
        Some("refs/heads/main"),
        &sig,
        &sig,
        "initial commit",
        &tree,
        &[],
    )?;
    repo.set_head("refs/heads/main")?;
    Ok(commit_oid)
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_mirror_clone_indexes_manifest() -> Result<()> {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    admin::create_tenant(&pool, "acme", "Acme", "team").await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
    let storage = Storage::from_uri(&storage_uri)?;

    // 1. Build an upstream bare repo with a valid plugin manifest.
    let upstream_dir = tempfile::tempdir()?;
    let upstream_path = upstream_dir.path().join("my-plugin.git");
    let manifest = json!({
        "name": "my-plugin",
        "version": "0.1.0",
        "description": "A test mirror plugin",
    });
    make_upstream_repo(&upstream_path, &manifest)?;
    let upstream_url = format!("file://{}", upstream_path.display());

    // 2. Insert the plugin stub row.
    let tenant_id: uuid::Uuid =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM tenants WHERE slug = 'acme'")
            .fetch_one(&pool)
            .await?;

    let plugin_id: uuid::Uuid = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO plugins \
             (tenant_id, slug, version, name, description, manifest, \
              status, sourcing_mode, upstream_url) \
         VALUES ($1, 'my-plugin', 'pending', 'my-plugin', NULL, '{}', \
                 'draft', 'mirror', $2) \
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(&upstream_url)
    .fetch_one(&pool)
    .await?;

    // 3. Run the mirror job directly.
    let job = PluginMirrorJob {
        plugin_id,
        tenant_id,
        upstream_url: upstream_url.clone(),
    };
    run_mirror(&pool, &storage, &job).await?;

    // 4. Assert: plugin row was updated.
    let (name, version, sourcing_mode, last_pulled_at, fetch_error) = sqlx::query_as::<
        _,
        (
            String,
            String,
            String,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<String>,
        ),
    >(
        "SELECT name, version, sourcing_mode, last_pulled_at, fetch_error \
         FROM plugins WHERE id = $1",
    )
    .bind(plugin_id)
    .fetch_one(&pool)
    .await?;

    assert_eq!(name, "my-plugin");
    assert_eq!(version, "0.1.0");
    assert_eq!(sourcing_mode, "mirror");
    assert!(
        last_pulled_at.is_some(),
        "last_pulled_at should be set after successful mirror"
    );
    assert!(
        fetch_error.is_none(),
        "fetch_error should be NULL after successful mirror"
    );

    // 5. Assert: marketplace entry exists.
    let entry_count: i64 = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM plugin_marketplace_entries WHERE plugin_id = $1",
    )
    .bind(plugin_id)
    .fetch_one(&pool)
    .await?;
    assert_eq!(entry_count, 1, "marketplace entry should exist");

    // 6. Assert: local bare repo contains the manifest.
    let repo_path = storage.plugin_git_path(tenant_id, "my-plugin")?;
    assert!(
        repo_path.exists(),
        "local bare repo should exist at {}",
        repo_path.display()
    );

    let local_repo = git2::Repository::open_bare(&repo_path)?;
    let head = local_repo.head()?.peel_to_commit()?;
    let tree = head.tree()?;
    let entry = tree.get_path(std::path::Path::new(".claude-plugin/plugin.json"))?;
    let blob = local_repo.find_blob(entry.id())?;
    let local_manifest: serde_json::Value = serde_json::from_slice(blob.content())?;
    assert_eq!(local_manifest["name"], "my-plugin");
    assert_eq!(local_manifest["version"], "0.1.0");

    Ok(())
}
