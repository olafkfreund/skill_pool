//! End-to-end smoke for per-tenant session-idle-timeout policy.
//!
//! 1. Boot the server, seed one tenant (`acme`) with default policy.
//! 2. GET /v1/tenant/session-policy → default 14 days, configured=false.
//! 3. Use the admin helper to set 1 hour.
//! 4. GET again → 3600 seconds, configured=true.
//! 5. Clear → back to default, configured=false.
//! 6. CHECK constraint rejection: 30 seconds (under 60s minimum) → error.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

#[tokio::test]
async fn per_tenant_session_policy_round_trip() -> Result<()> {
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

    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
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
    let base = format!("http://{addr}");
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // 1. Default — no policy yet.
    let resp = c
        .get(format!("{base}/v1/tenant/session-policy"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let body: serde_json::Value = resp.json().await?;
    assert_eq!(body["max_age_secs"], 14 * 24 * 60 * 60);
    assert_eq!(body["configured"], false);

    // 2. Set 1 hour.
    admin::set_session_max_age(&pool, "acme", Some(3600)).await?;
    let resp = c
        .get(format!("{base}/v1/tenant/session-policy"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    assert_eq!(body["max_age_secs"], 3600);
    assert_eq!(body["configured"], true);

    // 3. Clear.
    admin::set_session_max_age(&pool, "acme", None).await?;
    let resp = c
        .get(format!("{base}/v1/tenant/session-policy"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    let body: serde_json::Value = resp.json().await?;
    assert_eq!(body["max_age_secs"], 14 * 24 * 60 * 60);
    assert_eq!(body["configured"], false);

    // 4. CHECK constraint: 30s is below the 60s floor.
    let err = admin::set_session_max_age(&pool, "acme", Some(30))
        .await
        .expect_err("CHECK should reject 30s");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("range 60..2592000") || msg.to_lowercase().contains("check"),
        "expected CHECK violation, got: {msg}"
    );

    // 5. CHECK constraint: 31 days is above the 30-day ceiling.
    let err = admin::set_session_max_age(&pool, "acme", Some(31 * 24 * 60 * 60))
        .await
        .expect_err("CHECK should reject 31 days");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("range 60..2592000") || msg.to_lowercase().contains("check"),
        "expected CHECK violation, got: {msg}"
    );

    Ok(())
}
