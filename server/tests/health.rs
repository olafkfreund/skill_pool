//! Integration tests for `GET /v1/healthz` real dependency probes.
//!
//! Brings up Postgres via testcontainers (same pattern as integration.rs),
//! uses a tempdir for FS storage, and asserts the three dep probes behave
//! correctly. No embedder config → NullEmbedder → status "off".

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

struct Harness {
    base: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot() -> Result<Harness> {
    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let pg_port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{pg_port}/postgres");

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
        redis_url: None,
        db_pool_size: 20,
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
        queue_enabled: None,
        decay_check_interval_secs: 0,
        git_repo_path: None,
    };
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

#[tokio::test]
async fn healthz_dep_probes() -> Result<()> {
    let h = boot().await?;
    let client = reqwest::Client::new();

    let body: Value = client
        .get(format!("{}/v1/healthz", h.base))
        .send()
        .await?
        .json()
        .await?;

    // Top-level fields preserved for backward compatibility.
    assert_eq!(body["status"], "ok", "top-level status: {body}");
    assert!(
        body["version"].as_str().is_some(),
        "version must be a string: {body}"
    );

    // DB probe: working Postgres → "up" with numeric latency.
    let db = &body["deps"]["db"];
    assert_eq!(db["status"], "up", "db status: {db}");
    assert!(
        db["latency_ms"].as_u64().is_some(),
        "db latency_ms must be numeric: {db}"
    );

    // Storage probe: fs:// backend → stat("") succeeds → "up".
    let storage = &body["deps"]["storage"];
    assert_eq!(storage["status"], "up", "storage status: {storage}");
    assert!(
        storage["latency_ms"].as_u64().is_some(),
        "storage latency_ms must be numeric: {storage}"
    );

    // Embedder probe: NullEmbedder (default, no feature flag) → "off".
    let embedder = &body["deps"]["embedder"];
    assert_eq!(
        embedder["status"], "off",
        "embedder status with NullEmbedder: {embedder}"
    );
    // NullEmbedder must not report latency.
    assert!(
        embedder["latency_ms"].is_null(),
        "off embedder must not have latency_ms: {embedder}"
    );

    // The old top-level `db` string key must be absent (clients should read deps.db.status).
    assert!(
        body.get("db").is_none(),
        "old top-level `db` key must be removed: {body}"
    );

    Ok(())
}
