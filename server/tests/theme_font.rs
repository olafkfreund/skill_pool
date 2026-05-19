//! Integration test for the per-tenant font picker allowlist (issue #9).
//!
//! Covers:
//!   1. PUT /v1/theme with `font_family: "Inter"` → 200, value persisted on
//!      a subsequent GET.
//!   2. PUT /v1/theme with an out-of-allowlist family ("Comic Sans MS") →
//!      400 whose body mentions the allowlist.
//!   3. GET /v1/theme/fonts → 200, exposes the curated list (12 entries).

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

struct Harness {
    base: String,
    admin_token: String,
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

    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    let admin_token = admin::create_token(
        &pool,
        "acme",
        "admin",
        "tenant:admin skills:read skills:publish",
    )
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
        admin_token,
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

/// Build a complete Theme body with a configurable `font_family`. We always
/// pass the full set of required fields so a single PUT exercises validation
/// end-to-end (the route uses `Json<Theme>` which is non-partial).
fn theme_body(font: Option<&str>) -> Value {
    let mut v = json!({
        "brand_name": "acme",
        "primary": "#2563eb",
        "primary_fg": "#ffffff",
        "accent": "#0ea5e9",
        "bg": "#ffffff",
        "fg": "#0f172a",
        "muted": "#f1f5f9",
        "muted_fg": "#475569",
        "border": "#e2e8f0",
        "radius": "0.5rem",
        "footer_branding": true,
    });
    if let Some(f) = font {
        v["font_family"] = json!(f);
    }
    v
}

#[tokio::test]
async fn put_theme_with_allowed_font_persists() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = c
        .put(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .json(&theme_body(Some("Inter")))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "PUT with allowed font failed: {body}");
    assert_eq!(body["font_family"], "Inter");

    // GET round-trips the value.
    let resp = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["font_family"], "Inter");

    Ok(())
}

#[tokio::test]
async fn put_theme_with_disallowed_font_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = c
        .put(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .json(&theme_body(Some("Comic Sans MS")))
        .send()
        .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 400, "PUT with bad font should be 400: {body}");
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.contains("Comic Sans MS"),
        "error should name the rejected font: {msg}"
    );
    assert!(
        msg.to_ascii_lowercase().contains("allowlist"),
        "error should reference the allowlist: {msg}"
    );

    Ok(())
}

#[tokio::test]
async fn get_fonts_returns_allowlist() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = c
        .get(format!("{}/v1/theme/fonts", h.base))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await?;
    let allowed = body["allowed"]
        .as_array()
        .expect("allowed should be an array");
    assert_eq!(
        allowed.len(),
        12,
        "allowlist should have exactly 12 entries: {body}"
    );
    let names: Vec<&str> = allowed.iter().filter_map(|v| v.as_str()).collect();
    assert!(names.contains(&"system"), "system stack should be present");
    assert!(names.contains(&"Inter"), "Inter should be present");
    Ok(())
}
