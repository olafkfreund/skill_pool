//! Issue #32 — plugin mirror failure: unreachable URL records fetch_error.
//!
//! Flow:
//!   1. Boot Postgres.
//!   2. Insert a mirror plugin stub pointing at a URL that doesn't exist
//!      (`file:///nonexistent/path/plugin.git`).
//!   3. Call `run_mirror` directly — it should return Err.
//!   4. Call the handler (which calls run_mirror and writes the error back).
//!   5. Assert:
//!        - `fetch_error` is NOT NULL and non-empty.
//!        - `fetch_error_at` is NOT NULL.
//!        - `last_pulled_at` is still NULL (the failure didn't advance it).
//!        - The plugin row name/version are unchanged.

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{
    admin,
    jobs::plugin_mirror::{run_mirror, PluginMirrorHandler, PluginMirrorJob},
    storage::Storage,
    worker::JobHandler,
};

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plugin_mirror_failure_records_fetch_error() -> Result<()> {
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

    let tenant_id: uuid::Uuid = sqlx::query_scalar::<_, uuid::Uuid>(
        "SELECT id FROM tenants WHERE slug = 'acme'",
    )
    .fetch_one(&pool)
    .await?;

    // A URL that is guaranteed to fail.
    let bad_url = "file:///nonexistent/skill_pool_test/plugin.git";

    let plugin_id: uuid::Uuid = sqlx::query_scalar::<_, uuid::Uuid>(
        "INSERT INTO plugins \
             (tenant_id, slug, version, name, description, manifest, \
              status, sourcing_mode, upstream_url) \
         VALUES ($1, 'broken-mirror', 'pending', 'broken-mirror', NULL, '{}', \
                 'draft', 'mirror', $2) \
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(bad_url)
    .fetch_one(&pool)
    .await?;

    let job = PluginMirrorJob {
        plugin_id,
        tenant_id,
        upstream_url: bad_url.to_string(),
    };

    // 1. run_mirror should return Err.
    let result = run_mirror(&pool, &storage, &job).await;
    assert!(result.is_err(), "run_mirror should fail for an unreachable URL");

    // 2. Exercise the full handler path — which writes fetch_error to DB.
    let handler = PluginMirrorHandler::new(pool.clone(), storage.clone());
    let payload = serde_json::to_value(&job)?;
    let handle_result = handler.handle(payload).await;
    assert!(handle_result.is_err(), "handler should return Err for bad URL");

    // 3. Assert: fetch_error is populated, last_pulled_at is still NULL.
    let (name, version, last_pulled_at, fetch_error, fetch_error_at) = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<chrono::DateTime<chrono::Utc>>,
            Option<String>,
            Option<chrono::DateTime<chrono::Utc>>,
        ),
    >(
        "SELECT name, version, last_pulled_at, fetch_error, fetch_error_at \
         FROM plugins WHERE id = $1",
    )
    .bind(plugin_id)
    .fetch_one(&pool)
    .await?;

    assert!(
        last_pulled_at.is_none(),
        "last_pulled_at must remain NULL after a failed mirror"
    );
    assert!(
        fetch_error.is_some(),
        "fetch_error must be populated after a failed mirror"
    );
    assert!(
        fetch_error_at.is_some(),
        "fetch_error_at must be set after a failed mirror"
    );

    let err_msg = fetch_error.unwrap();
    assert!(
        !err_msg.is_empty(),
        "fetch_error should contain a non-empty error message"
    );

    // Row fields must be intact.
    assert_eq!(name, "broken-mirror");
    assert_eq!(version, "pending");

    Ok(())
}
