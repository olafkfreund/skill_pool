//! Issue #30 — confirm the per-tenant rate limiter covers the new
//! `/v1/plugins` surface (the limiter is a workspace-wide tower layer;
//! this test asserts the wiring catches the new routes too).
//!
//! Mirrors `tests/rate_limits.rs::per_tenant_rate_limit_429_after_threshold`
//! but hits `/v1/plugins` instead of `/v1/skills`.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;

use skill_pool_server::{admin, cache, config, routes, state};

#[tokio::test]
async fn rate_limiter_covers_plugins_routes() -> Result<()> {
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

    let rd = Redis::default().start().await?;
    let rd_port = rd.get_host_port_ipv4(6379).await?;
    let redis_url = format!("redis://127.0.0.1:{rd_port}");
    let redis = cache::connect(&redis_url).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    // Tight cap so a single-threaded hammer drives past quickly.
    admin::set_tenant_rate_limits(&pool, "acme", Some(5), Some(5), false).await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
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
        queue_enabled: None,
        decay_check_interval_secs: 0,
        git_repo_path: None,
    };
    let app_state = state::AppState::new_with_redis(&cfg, redis.clone()).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    let mut allowed = 0;
    let mut denied = 0;
    for i in 0..10 {
        let resp = c
            .get(format!("{base}/v1/plugins"))
            .header("x-skill-pool-tenant", "acme")
            .header("authorization", format!("Bearer {acme_token}"))
            .send()
            .await?;
        match resp.status().as_u16() {
            429 => denied += 1,
            200 => allowed += 1,
            s => panic!("unexpected status {s} on request {i}"),
        }
    }
    assert_eq!(allowed, 5, "expected 5 allowed /v1/plugins requests");
    assert_eq!(denied, 5, "expected 5 throttled /v1/plugins requests");

    drop(pool);
    Ok(())
}
