//! Integration test for the per-tenant favicon endpoint (issue #9).
//!
//! Mirrors `theme_logo.rs` and covers the favicon-specific behaviours:
//!   1. POST + GET roundtrip: 200, correct content-type, cache-control.
//!   2. No favicon AND no logo → 404.
//!   3. No favicon but logo set → 200 with the logo bytes (the fallback).
//!   4. Oversized payload (>64 KiB) → 400.
//!   5. DELETE → 204; subsequent GET falls back to logo (or 404 with no logo).
//!
//! The "logo fallback" path is the only really novel bit vs. theme_logo.rs.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use reqwest::multipart::{Form, Part};
use serde_json::Value;
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

fn good_svg() -> Vec<u8> {
    br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
  <rect width="16" height="16" fill="#2563eb"/>
</svg>"##
        .to_vec()
}

fn good_logo_svg() -> Vec<u8> {
    br##"<?xml version="1.0"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <rect width="24" height="24" fill="#0ea5e9"/>
  <text x="12" y="14" font-size="6" text-anchor="middle" fill="#fff">LG</text>
</svg>"##
        .to_vec()
}

async fn upload_favicon(
    c: &reqwest::Client,
    h: &Harness,
    bytes: Vec<u8>,
    content_type: &str,
    filename: &str,
) -> Result<reqwest::Response> {
    let part = Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(content_type)?;
    let form = Form::new().part("file", part);
    let resp = c
        .post(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .multipart(form)
        .send()
        .await?;
    Ok(resp)
}

async fn upload_logo(
    c: &reqwest::Client,
    h: &Harness,
    bytes: Vec<u8>,
    content_type: &str,
    filename: &str,
) -> Result<reqwest::Response> {
    let part = Part::bytes(bytes)
        .file_name(filename.to_string())
        .mime_str(content_type)?;
    let form = Form::new().part("file", part);
    let resp = c
        .post(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .multipart(form)
        .send()
        .await?;
    Ok(resp)
}

#[tokio::test]
async fn favicon_upload_get_roundtrip() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = upload_favicon(&c, &h, good_svg(), "image/svg+xml", "favicon.svg").await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "valid favicon upload failed: {body}");
    assert_eq!(body["brand_name"], "acme");

    let resp = c
        .get(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/svg+xml"),
    );
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=300"),
    );
    let returned = resp.bytes().await?;
    let s = String::from_utf8_lossy(&returned);
    assert!(s.contains("<svg"), "favicon body should contain <svg: {s}");

    Ok(())
}

#[tokio::test]
async fn favicon_missing_with_no_logo_returns_404() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = c
        .get(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        404,
        "no favicon AND no logo should be 404"
    );
    Ok(())
}

#[tokio::test]
async fn favicon_falls_back_to_logo_when_unset() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // Upload a logo, then GET favicon — the logo bytes should come back.
    let resp = upload_logo(&c, &h, good_logo_svg(), "image/svg+xml", "logo.svg").await?;
    assert_eq!(resp.status().as_u16(), 200, "logo upload should succeed");

    let resp = c
        .get(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "favicon GET should fall back to logo"
    );
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("image/svg+xml"),
        "favicon fallback should serve the logo's content-type",
    );
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=300"),
    );
    let body = resp.bytes().await?;
    let s = String::from_utf8_lossy(&body);
    // The logo's SVG carries the distinctive `LG` glyph; favicons start with
    // the very different SP glyph. Cross-checking the body lets us prove this
    // is the logo, not a stale favicon.
    assert!(s.contains("LG"), "fallback should be the logo bytes: {s}");

    Ok(())
}

#[tokio::test]
async fn oversized_favicon_is_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 70 KiB of zeros — well over the 64 KiB cap. Claim PNG so the
    // content-type check passes; the size check fires first anyway.
    let big = vec![0u8; 70 * 1024];
    let resp = upload_favicon(&c, &h, big, "image/png", "huge.png").await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 400, "oversized favicon should be 400: {body}");
    let msg = body["message"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(
        msg.contains("large") || msg.contains("size"),
        "error should mention size: {msg}"
    );

    Ok(())
}

#[tokio::test]
async fn favicon_delete_falls_back_to_logo_then_404() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 1. Upload favicon AND logo.
    let resp = upload_favicon(&c, &h, good_svg(), "image/svg+xml", "favicon.svg").await?;
    assert_eq!(resp.status().as_u16(), 200);
    let resp = upload_logo(&c, &h, good_logo_svg(), "image/svg+xml", "logo.svg").await?;
    assert_eq!(resp.status().as_u16(), 200);

    // 2. DELETE favicon → 204.
    let resp = c
        .delete(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204);

    // 3. GET favicon should now serve the logo (fallback).
    let resp = c
        .get(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "after favicon delete the logo should be served"
    );
    let body = resp.bytes().await?;
    let s = String::from_utf8_lossy(&body);
    assert!(s.contains("LG"), "should be the logo bytes: {s}");

    // 4. Now delete the logo too → favicon GET should 404.
    let resp = c
        .delete(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204);

    let resp = c
        .get(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}

#[tokio::test]
async fn favicon_upload_without_auth_is_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let part = Part::bytes(good_svg())
        .file_name("favicon.svg")
        .mime_str("image/svg+xml")?;
    let form = Form::new().part("file", part);
    let resp = c
        .post(format!("{}/v1/theme/favicon", h.base))
        .header("x-skill-pool-tenant", "acme")
        .multipart(form)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 401);

    Ok(())
}
