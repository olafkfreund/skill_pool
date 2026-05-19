//! Integration test for GET /metrics.
//!
//! Boots the full stack (Postgres via testcontainers + FS storage) on an
//! ephemeral port, fires a handful of requests at real API endpoints, then
//! scrapes `/metrics` and asserts that `http_requests_total` is non-zero and
//! that the response carries the correct `Content-Type`.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

// ---------------------------------------------------------------------------
// Shared harness (subset of the one in integration.rs)
// ---------------------------------------------------------------------------

struct Harness {
    base: String,
    token: String,
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

    admin::create_tenant(&pool, "metrics-test", "Metrics Test", "team").await?;
    let token =
        admin::create_token(&pool, "metrics-test", "test", "skills:read skills:publish")
            .await?
            .raw_token;

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
        token,
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn metrics_endpoint_returns_non_zero_counter() -> Result<()> {
    let h = boot().await?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    // Fire a few requests so the counter is definitely > 0.
    for _ in 0..3 {
        let resp = client
            .get(format!("{}/v1/healthz", h.base))
            .header("x-skill-pool-tenant", "metrics-test")
            .bearer_auth(&h.token)
            .send()
            .await?;
        assert!(resp.status().is_success(), "healthz failed: {}", resp.status());
    }

    // Scrape /metrics.
    let resp = client
        .get(format!("{}/metrics", h.base))
        .send()
        .await?;

    assert_eq!(resp.status(), 200, "unexpected status from /metrics");

    // Content-Type must be Prometheus text exposition format 0.0.4.
    let ct = resp
        .headers()
        .get("content-type")
        .expect("content-type header missing")
        .to_str()?;
    assert!(
        ct.contains("text/plain") && ct.contains("0.0.4"),
        "unexpected Content-Type: {ct}"
    );

    let body = resp.text().await?;

    // The counter family must be present.
    assert!(
        body.contains("http_requests_total"),
        "http_requests_total not found in /metrics output"
    );

    // At least one counter value must be non-zero.
    let has_nonzero = body
        .lines()
        .filter(|l| l.starts_with("http_requests_total{"))
        .any(|l| {
            l.rsplit_once(' ')
                .and_then(|(_, v)| v.trim().parse::<f64>().ok())
                .map(|v| v > 0.0)
                .unwrap_or(false)
        });
    assert!(has_nonzero, "all http_requests_total counters are zero");

    // The histogram family must also be present.
    assert!(
        body.contains("http_request_duration_seconds"),
        "http_request_duration_seconds not found in /metrics output"
    );

    Ok(())
}
