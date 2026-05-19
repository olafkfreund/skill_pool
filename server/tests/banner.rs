//! Per-tenant CLI startup banner (#9) — round-trip integration test.
//!
//! 1. Boot server with one tenant; GET banner → both null.
//! 2. Set text + url via admin helper; GET → both echoed.
//! 3. Update text only (leave url alone) → url stays previous value
//!    (proves the CASE expression "leave unchanged" branch).
//! 4. Update url only → text stays previous value.
//! 5. Clear both via `clear = true` → both null again.
//! 6. CHECK constraints: 241-char text rejected; non-https url rejected.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

#[tokio::test]
async fn per_tenant_banner_round_trip() -> Result<()> {
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

    // 1. Default: both null.
    let body: serde_json::Value = c
        .get(format!("{base}/v1/tenant/profile/banner"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert!(body["text"].is_null(), "expected null text, got {body:?}");
    assert!(body["url"].is_null(), "expected null url, got {body:?}");

    // 2. Set both.
    admin::set_tenant_banner(
        &pool,
        "acme",
        Some("Welcome to Acme"),
        Some("https://wiki.acme.example.com/skills"),
        false,
    )
    .await?;
    let body: serde_json::Value = c
        .get(format!("{base}/v1/tenant/profile/banner"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(body["text"], "Welcome to Acme");
    assert_eq!(body["url"], "https://wiki.acme.example.com/skills");

    // 3. Update text only — url should stay.
    admin::set_tenant_banner(&pool, "acme", Some("Maintenance Saturday"), None, false).await?;
    let body: serde_json::Value = c
        .get(format!("{base}/v1/tenant/profile/banner"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(body["text"], "Maintenance Saturday");
    assert_eq!(
        body["url"], "https://wiki.acme.example.com/skills",
        "url should be untouched when only text is updated"
    );

    // 4. Update url only — text should stay.
    admin::set_tenant_banner(
        &pool,
        "acme",
        None,
        Some("https://status.acme.example.com"),
        false,
    )
    .await?;
    let body: serde_json::Value = c
        .get(format!("{base}/v1/tenant/profile/banner"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(body["text"], "Maintenance Saturday");
    assert_eq!(body["url"], "https://status.acme.example.com");

    // 5. Clear both.
    admin::set_tenant_banner(&pool, "acme", None, None, true).await?;
    let body: serde_json::Value = c
        .get(format!("{base}/v1/tenant/profile/banner"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert!(body["text"].is_null());
    assert!(body["url"].is_null());

    // 6a. CHECK constraint: 241-char text → error.
    let huge = "a".repeat(241);
    let err = admin::set_tenant_banner(&pool, "acme", Some(&huge), None, false)
        .await
        .expect_err("CHECK should reject 241-char banner_text");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("check") || msg.contains("≤240") || msg.contains("240"),
        "expected CHECK violation, got: {msg}"
    );

    // 6b. CHECK constraint: non-https url → error.
    let err = admin::set_tenant_banner(
        &pool,
        "acme",
        None,
        Some("http://insecure.example.com"),
        false,
    )
    .await
    .expect_err("CHECK should reject non-https url");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("check") || msg.contains("https"),
        "expected CHECK violation, got: {msg}"
    );

    // 6c. CHECK constraint: url with whitespace → error.
    let err = admin::set_tenant_banner(
        &pool,
        "acme",
        None,
        Some("https://example.com/ has space"),
        false,
    )
    .await
    .expect_err("CHECK should reject url with whitespace");
    let msg = format!("{err:#}").to_lowercase();
    assert!(
        msg.contains("check") || msg.contains("https"),
        "expected CHECK violation, got: {msg}"
    );

    Ok(())
}
