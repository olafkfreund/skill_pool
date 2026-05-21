//! Issue #29 — schema tests for plugins + plugin_contents +
//! plugin_marketplace_entries (migrations 0031 + 0032).
//!
//! Boots a fresh pgvector container per test (same pattern as
//! `catalog_kinds.rs`). Each `#[tokio::test]` covers one invariant so
//! a failure points straight at the broken constraint.

use anyhow::Result;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

use skill_pool_server::admin;

struct Db {
    pool: PgPool,
    _pg: testcontainers::ContainerAsync<Postgres>,
}

async fn fresh_db() -> Result<Db> {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let port = pg.get_host_port_ipv4(5432).await?;
    let url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new().max_connections(4).connect(&url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;
    Ok(Db { pool, _pg: pg })
}

async fn make_tenant(pool: &PgPool, slug: &str) -> Result<Uuid> {
    Ok(admin::create_tenant(pool, slug, slug, "team").await?.id)
}

async fn insert_plugin(
    pool: &PgPool,
    tenant_id: Uuid,
    slug: &str,
    version: &str,
) -> Result<Uuid> {
    let row = sqlx::query!(
        "INSERT INTO plugins (tenant_id, slug, version, name, manifest) \
         VALUES ($1, $2, $3, $4, $5) RETURNING id",
        tenant_id,
        slug,
        version,
        slug,
        json!({ "name": slug, "version": version }),
    )
    .fetch_one(pool)
    .await?;
    Ok(row.id)
}

// ---------------------------------------------------------------------------
// 1. Migration idempotency
// ---------------------------------------------------------------------------

#[tokio::test]
async fn migrations_are_idempotent() -> Result<()> {
    let db = fresh_db().await?;
    // fresh_db already ran migrations once; run again and assert clean.
    sqlx::migrate!("./migrations").run(&db.pool).await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Tenant cascade across all three new tables
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tenant_cascade_deletes_plugins_and_contents() -> Result<()> {
    let db = fresh_db().await?;
    let acme = make_tenant(&db.pool, "acme").await?;
    let globex = make_tenant(&db.pool, "globex").await?;

    let plugin_id = insert_plugin(&db.pool, acme, "rust-toolkit", "1.0.0").await?;

    sqlx::query!(
        "INSERT INTO plugin_contents \
           (plugin_id, content_slug, content_kind, content_version) \
         VALUES ($1, $2, $3, $4), ($1, $5, $6, $7)",
        plugin_id, "fmt", "skill", "0.1.0",
        "lint", "agent", "0.2.0",
    )
    .execute(&db.pool)
    .await?;

    sqlx::query!(
        "INSERT INTO plugin_marketplace_entries \
           (tenant_id, plugin_slug, plugin_id, version, source_url, entry_json) \
         VALUES ($1, $2, $3, $4, $5, $6)",
        acme, "rust-toolkit", plugin_id, "1.0.0",
        "https://acme.skill-pool.example.com/git/plugins/rust-toolkit.git",
        json!({ "name": "rust-toolkit", "version": "1.0.0" }),
    )
    .execute(&db.pool)
    .await?;

    // Sanity: globex sees nothing yet.
    let n: (i64,) = sqlx::query_as("SELECT count(*) FROM plugins WHERE tenant_id = $1")
        .bind(globex)
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(n.0, 0);

    sqlx::query!("DELETE FROM tenants WHERE id = $1", acme)
        .execute(&db.pool)
        .await?;

    let plugins: (i64,) = sqlx::query_as("SELECT count(*) FROM plugins WHERE tenant_id = $1")
        .bind(acme)
        .fetch_one(&db.pool)
        .await?;
    assert_eq!(plugins.0, 0, "plugins should cascade-delete with tenant");

    let contents: (i64,) =
        sqlx::query_as("SELECT count(*) FROM plugin_contents WHERE plugin_id = $1")
            .bind(plugin_id)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(contents.0, 0, "plugin_contents should cascade via plugin_id");

    let entries: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM plugin_marketplace_entries WHERE tenant_id = $1",
    )
    .bind(acme)
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(entries.0, 0, "marketplace entries should cascade with tenant");

    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Cross-tenant plugin_id mismatch — documents the API-layer gap
// ---------------------------------------------------------------------------
//
// The FK on `plugin_marketplace_entries.plugin_id` is to `plugins.id`
// alone (not composite with tenant). That means the schema PERMITS a row
// whose `tenant_id` differs from the referenced plugin's tenant. This is
// intentional (default plan): defense-in-depth lives in the API handler
// landing in #2. This test pins that reality so a future schema tweak
// doesn't silently change behaviour.

#[tokio::test]
async fn cross_tenant_plugin_id_in_marketplace_entry_is_api_layer_concern() -> Result<()> {
    let db = fresh_db().await?;
    let acme = make_tenant(&db.pool, "acme").await?;
    let globex = make_tenant(&db.pool, "globex").await?;

    let acme_plugin = insert_plugin(&db.pool, acme, "shared", "1.0.0").await?;

    // Insert a marketplace entry with globex's tenant_id but acme's plugin.
    // We expect this to SUCCEED at the schema layer — the API handler is
    // the layer that must reject it.
    let res = sqlx::query!(
        "INSERT INTO plugin_marketplace_entries \
           (tenant_id, plugin_slug, plugin_id, version, source_url, entry_json) \
         VALUES ($1, $2, $3, $4, $5, $6)",
        globex, "shared", acme_plugin, "1.0.0",
        "https://example.com/git.git", json!({}),
    )
    .execute(&db.pool)
    .await;

    assert!(
        res.is_ok(),
        "schema permits cross-tenant plugin_id — API layer (#2) MUST reject. \
         If this assertion ever flips, update the doc in 0032 and the API \
         handler's `cross_tenant_check` to match."
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 4. CHECK constraint on plugin_contents.content_kind
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_contents_kind_check_rejects_bogus() -> Result<()> {
    let db = fresh_db().await?;
    let acme = make_tenant(&db.pool, "acme").await?;
    let plugin = insert_plugin(&db.pool, acme, "kit", "1.0.0").await?;

    let err = sqlx::query!(
        "INSERT INTO plugin_contents \
           (plugin_id, content_slug, content_kind, content_version) \
         VALUES ($1, $2, $3, $4)",
        plugin, "bogus", "plugin", "1.0.0",
    )
    .execute(&db.pool)
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(
        msg.contains("plugin_contents_content_kind_check") || msg.contains("check constraint"),
        "expected CHECK violation, got: {msg}"
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// 5. UNIQUE (tenant_id, slug, version) on plugins
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_slug_version_unique_per_tenant() -> Result<()> {
    let db = fresh_db().await?;
    let acme = make_tenant(&db.pool, "acme").await?;
    let globex = make_tenant(&db.pool, "globex").await?;

    insert_plugin(&db.pool, acme, "kit", "1.0.0").await?;

    // Same tenant + slug + version → unique violation.
    let dup = insert_plugin(&db.pool, acme, "kit", "1.0.0").await;
    assert!(dup.is_err(), "duplicate (tenant, slug, version) must fail");
    let msg = dup.unwrap_err().to_string();
    assert!(
        msg.contains("plugins_tenant_id_slug_version_key") || msg.contains("duplicate key"),
        "expected unique-violation, got: {msg}"
    );

    // Different tenant, same (slug, version) → must succeed (proves scoping).
    insert_plugin(&db.pool, globex, "kit", "1.0.0").await?;

    // Same tenant, same slug, different version → must succeed.
    insert_plugin(&db.pool, acme, "kit", "1.1.0").await?;

    Ok(())
}

// ---------------------------------------------------------------------------
// 6. Plugin cascade → plugin_contents + plugin_marketplace_entries
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_cascade_deletes_contents_and_entries() -> Result<()> {
    let db = fresh_db().await?;
    let acme = make_tenant(&db.pool, "acme").await?;
    let plugin = insert_plugin(&db.pool, acme, "kit", "1.0.0").await?;

    sqlx::query!(
        "INSERT INTO plugin_contents \
           (plugin_id, content_slug, content_kind, content_version) \
         VALUES ($1, $2, $3, $4)",
        plugin, "fmt", "skill", "0.1.0",
    )
    .execute(&db.pool)
    .await?;

    sqlx::query!(
        "INSERT INTO plugin_marketplace_entries \
           (tenant_id, plugin_slug, plugin_id, version, source_url, entry_json) \
         VALUES ($1, $2, $3, $4, $5, $6)",
        acme, "kit", plugin, "1.0.0", "https://example.com/g.git", json!({}),
    )
    .execute(&db.pool)
    .await?;

    sqlx::query!("DELETE FROM plugins WHERE id = $1", plugin)
        .execute(&db.pool)
        .await?;

    let contents: (i64,) =
        sqlx::query_as("SELECT count(*) FROM plugin_contents WHERE plugin_id = $1")
            .bind(plugin)
            .fetch_one(&db.pool)
            .await?;
    assert_eq!(contents.0, 0);

    let entries: (i64,) = sqlx::query_as(
        "SELECT count(*) FROM plugin_marketplace_entries WHERE plugin_id = $1",
    )
    .bind(plugin)
    .fetch_one(&db.pool)
    .await?;
    assert_eq!(entries.0, 0);

    Ok(())
}

// ---------------------------------------------------------------------------
// 7. sourcing_mode invariants (paired URL CHECK constraints)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn sourcing_mode_invariants() -> Result<()> {
    let db = fresh_db().await?;
    let acme = make_tenant(&db.pool, "acme").await?;

    // External without external_git_url → reject.
    let bad_external = sqlx::query!(
        "INSERT INTO plugins (tenant_id, slug, version, name, manifest, sourcing_mode) \
         VALUES ($1, $2, $3, $4, $5, 'external')",
        acme, "ext-no-url", "1.0.0", "ext-no-url", json!({}),
    )
    .execute(&db.pool)
    .await;
    assert!(bad_external.is_err(), "external without git url must fail");

    // Mirror without upstream_url → reject.
    let bad_mirror = sqlx::query!(
        "INSERT INTO plugins (tenant_id, slug, version, name, manifest, sourcing_mode) \
         VALUES ($1, $2, $3, $4, $5, 'mirror')",
        acme, "mir-no-url", "1.0.0", "mir-no-url", json!({}),
    )
    .execute(&db.pool)
    .await;
    assert!(bad_mirror.is_err(), "mirror without upstream url must fail");

    // Internal with both URLs NULL → accept.
    sqlx::query!(
        "INSERT INTO plugins (tenant_id, slug, version, name, manifest, sourcing_mode) \
         VALUES ($1, $2, $3, $4, $5, 'internal')",
        acme, "internal-ok", "1.0.0", "internal-ok", json!({}),
    )
    .execute(&db.pool)
    .await?;

    // External with URL → accept.
    sqlx::query!(
        "INSERT INTO plugins (tenant_id, slug, version, name, manifest, sourcing_mode, external_git_url) \
         VALUES ($1, $2, $3, $4, $5, 'external', $6)",
        acme, "external-ok", "1.0.0", "external-ok", json!({}),
        "https://github.com/foo/bar",
    )
    .execute(&db.pool)
    .await?;

    // Mirror with URL → accept.
    sqlx::query!(
        "INSERT INTO plugins (tenant_id, slug, version, name, manifest, sourcing_mode, upstream_url) \
         VALUES ($1, $2, $3, $4, $5, 'mirror', $6)",
        acme, "mirror-ok", "1.0.0", "mirror-ok", json!({}),
        "https://github.com/foo/upstream",
    )
    .execute(&db.pool)
    .await?;

    Ok(())
}
