//! End-to-end smoke for per-tenant rate-limiting (#8 §L20).
//!
//! Spins up Postgres + Redis via testcontainers, wires the router with
//! `AppState::new_with_redis`, sets a tight rpm cap on `acme` (rpm=5,
//! burst=5), and verifies:
//!
//!   1. The first 5 requests to `/v1/skills` succeed.
//!   2. Requests 6..10 return 429 with a `Retry-After` header and the
//!      `X-RateLimit-*` family populated.
//!   3. After the 60s window expires the limit resets — we don't wait
//!      the full minute in CI; instead we manually flush the Redis
//!      counter so the test runs in seconds. The window-rollover code
//!      path is covered by the unit tests in `rate_limit::tests`.
//!   4. The skip list works: `/v1/healthz` is never throttled even
//!      under the same hammering.
//!
//! Requires a Docker socket. Skipped automatically when one isn't
//! present (the boot helper returns Err).

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
async fn per_tenant_rate_limit_429_after_threshold() -> Result<()> {
    // ----- Postgres ---------------------------------------------------
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

    // ----- Redis ------------------------------------------------------
    let rd = Redis::default().start().await?;
    let rd_port = rd.get_host_port_ipv4(6379).await?;
    let redis_url = format!("redis://127.0.0.1:{rd_port}");
    let redis = cache::connect(&redis_url).await?;

    // ----- Seed: one tenant + token, RPM/burst clamped tight ---------
    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    // Cap at 5 rpm + 5 burst — small enough that we can drive past in a
    // handful of requests but big enough that the burst doesn't trip
    // first on a single-threaded loop.
    admin::set_tenant_rate_limits(&pool, "acme", Some(5), Some(5), false).await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    // ----- Server -----------------------------------------------------
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

    // ----- 1. First 5 succeed -----------------------------------------
    let mut allowed = 0;
    let mut denied = 0;
    let mut first_429: Option<reqwest::Response> = None;
    for i in 0..10 {
        let resp = c
            .get(format!("{base}/v1/skills"))
            .header("x-skill-pool-tenant", "acme")
            .header("authorization", format!("Bearer {acme_token}"))
            .send()
            .await?;
        match resp.status().as_u16() {
            429 => {
                denied += 1;
                if first_429.is_none() {
                    first_429 = Some(resp);
                }
            }
            200 => allowed += 1,
            s => panic!("unexpected status {s} on request {i}"),
        }
    }
    assert_eq!(
        allowed, 5,
        "expected exactly 5 allowed requests, got {allowed}"
    );
    assert_eq!(denied, 5, "expected 5 throttled requests, got {denied}");

    // ----- 2. The first 429 carries the headers the docs promise -----
    let resp429 = first_429.expect("at least one 429");
    let h = resp429.headers();
    assert!(
        h.get("retry-after").is_some(),
        "missing Retry-After on 429: {h:?}"
    );
    assert_eq!(
        h.get("x-ratelimit-limit").and_then(|v| v.to_str().ok()),
        Some("5")
    );
    assert_eq!(
        h.get("x-ratelimit-remaining").and_then(|v| v.to_str().ok()),
        Some("0")
    );
    assert!(
        h.get("x-ratelimit-reset").is_some(),
        "missing X-RateLimit-Reset"
    );

    // ----- 3. Skip list — /v1/healthz is never throttled --------------
    // Hammer the health endpoint past the cap; all must succeed.
    for _ in 0..20 {
        let resp = c.get(format!("{base}/v1/healthz")).send().await?;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "/v1/healthz must bypass limits"
        );
    }

    // ----- 4. Flushing the Redis counters resets the window ----------
    // (simulates the 60s rollover without sleeping).
    let mut conn = (*redis).clone();
    let _: () = redis::cmd("FLUSHDB").query_async(&mut conn).await?;
    let resp = c
        .get(format!("{base}/v1/skills"))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {acme_token}"))
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "request after window reset must succeed; got {} headers={:?}",
        resp.status(),
        resp.headers()
    );

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn rate_limit_fails_open_without_redis() -> Result<()> {
    // When Redis is unconfigured the middleware must be a no-op — every
    // request runs through. Boot the server with `AppState::new` (which
    // reads `SKILL_POOL_REDIS_URL`, unset in the test env) and verify a
    // burst beyond any sane cap all 200s.
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
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    admin::set_tenant_rate_limits(&pool, "acme", Some(1), Some(1), false).await?;
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
    // Explicitly clear SKILL_POOL_REDIS_URL so `AppState::new` sees no
    // Redis (the test runner may have set it).
    // SAFETY: we're a leaf integration test — no other thread reads env.
    unsafe { std::env::remove_var("SKILL_POOL_REDIS_URL") };
    let app_state = state::AppState::new(&cfg).await?;
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

    // Cap is 1/1 but Redis is offline → fail-open, every request 200.
    for i in 0..5 {
        let resp = c
            .get(format!("{base}/v1/skills"))
            .header("x-skill-pool-tenant", "acme")
            .header("authorization", format!("Bearer {acme_token}"))
            .send()
            .await?;
        assert_eq!(
            resp.status().as_u16(),
            200,
            "no-redis must fail-open, request {i} got {}",
            resp.status()
        );
    }

    drop(pool);
    Ok(())
}
