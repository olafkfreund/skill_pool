//! Issue #32 — plugin mirror refresh: upstream changes propagate + last_pulled_at advances.
//!
//! Flow:
//!   1. Boot Postgres.
//!   2. Build an upstream bare repo (v1 manifest).
//!   3. Insert plugin stub, run_mirror → verifies v1 is indexed.
//!   4. Mutate the upstream (add a v2 commit to `main`).
//!   5. Run run_mirror again — should fast-forward fetch and update the row.
//!   6. Assert:
//!        - `plugins.version` updated to "0.2.0".
//!        - `last_pulled_at` is >= the first run's timestamp.
//!        - The local bare repo HEAD now has the v2 manifest.

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

fn make_upstream_repo_v1(repo_path: &Path) -> Result<()> {
    use git2::{Repository, Signature};

    let manifest = json!({
        "name": "refresh-plugin",
        "version": "0.1.0",
        "description": "First version",
    });
    let repo = Repository::init_bare(repo_path)?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    let blob_oid = repo.blob(&manifest_bytes)?;
    let mut inner = repo.treebuilder(None)?;
    inner.insert("plugin.json", blob_oid, 0o100644)?;
    let inner_oid = inner.write()?;
    let mut root = repo.treebuilder(None)?;
    root.insert(".claude-plugin", inner_oid, 0o040000)?;
    let root_oid = root.write()?;
    let tree = repo.find_tree(root_oid)?;
    let sig = Signature::now("test", "test@example.com")?;
    repo.commit(Some("refs/heads/main"), &sig, &sig, "v1", &tree, &[])?;
    repo.set_head("refs/heads/main")?;
    Ok(())
}

fn push_upstream_v2(repo_path: &Path) -> Result<()> {
    use git2::{Repository, Signature};

    let manifest_v2 = json!({
        "name": "refresh-plugin",
        "version": "0.2.0",
        "description": "Second version",
    });
    let repo = Repository::open_bare(repo_path)?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest_v2)?;
    let blob_oid = repo.blob(&manifest_bytes)?;
    let mut inner = repo.treebuilder(None)?;
    inner.insert("plugin.json", blob_oid, 0o100644)?;
    let inner_oid = inner.write()?;
    let mut root = repo.treebuilder(None)?;
    root.insert(".claude-plugin", inner_oid, 0o040000)?;
    let root_oid = root.write()?;
    let tree = repo.find_tree(root_oid)?;
    let sig = Signature::now("test", "test@example.com")?;
    let parent = repo.head()?.peel_to_commit()?;
    repo.commit(Some("refs/heads/main"), &sig, &sig, "v2", &tree, &[&parent])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_mirror_refresh_propagates_upstream_changes() -> Result<()> {
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

    // Build upstream v1.
    let upstream_dir = tempfile::tempdir()?;
    let upstream_path = upstream_dir.path().join("refresh-plugin.git");
    make_upstream_repo_v1(&upstream_path)?;
    let upstream_url = format!("file://{}", upstream_path.display());

    let tenant_id: uuid::Uuid =
        sqlx::query_scalar::<_, uuid::Uuid>("SELECT id FROM tenants WHERE slug = 'acme'")
            .fetch_one(&pool)
            .await?;

    let plugin_id: uuid::Uuid = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO plugins \
             (tenant_id, slug, version, name, description, manifest, \
              status, sourcing_mode, upstream_url) \
         VALUES ($1, 'refresh-plugin', 'pending', 'refresh-plugin', NULL, '{}', \
                 'draft', 'mirror', $2) \
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(&upstream_url)
    .fetch_one(&pool)
    .await?;

    let job = PluginMirrorJob {
        plugin_id,
        tenant_id,
        upstream_url: upstream_url.clone(),
    };

    // First mirror run — should pick up v1.
    run_mirror(&pool, &storage, &job).await?;
    let first_pulled_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT last_pulled_at FROM plugins WHERE id = $1",
        )
        .bind(plugin_id)
        .fetch_one(&pool)
        .await?;
    let first_version: String =
        sqlx::query_scalar::<_, String>("SELECT version FROM plugins WHERE id = $1")
            .bind(plugin_id)
            .fetch_one(&pool)
            .await?;
    assert_eq!(first_version, "0.1.0");

    // Push v2 to the upstream.
    push_upstream_v2(&upstream_path)?;

    // Brief sleep so timestamps are distinguishable.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    // Second mirror run — should fast-forward and pick up v2.
    run_mirror(&pool, &storage, &job).await?;
    let second_pulled_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar::<_, chrono::DateTime<chrono::Utc>>(
            "SELECT last_pulled_at FROM plugins WHERE id = $1",
        )
        .bind(plugin_id)
        .fetch_one(&pool)
        .await?;
    let second_version: String =
        sqlx::query_scalar::<_, String>("SELECT version FROM plugins WHERE id = $1")
            .bind(plugin_id)
            .fetch_one(&pool)
            .await?;

    assert_eq!(
        second_version, "0.2.0",
        "version should update to 0.2.0 after refresh"
    );
    assert!(
        second_pulled_at >= first_pulled_at,
        "last_pulled_at should advance on refresh"
    );

    // Verify the local repo HEAD has the v2 manifest.
    let repo_path = storage.plugin_git_path(tenant_id, "refresh-plugin")?;
    let local_repo = git2::Repository::open_bare(&repo_path)?;
    let head_tree = local_repo.head()?.peel_to_tree()?;
    let entry = head_tree.get_path(std::path::Path::new(".claude-plugin/plugin.json"))?;
    let blob = local_repo.find_blob(entry.id())?;
    let manifest: serde_json::Value = serde_json::from_slice(blob.content())?;
    assert_eq!(
        manifest["version"], "0.2.0",
        "local repo should reflect upstream v2"
    );

    Ok(())
}
