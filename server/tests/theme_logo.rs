//! Integration test for the per-tenant logo upload pipeline (issue #9).
//!
//! Covers:
//!   1. Valid SVG upload → 200, theme reflects the new logo metadata.
//!   2. Malicious SVG with `<script>alert(1)</script>` → 400.
//!   3. Oversized file → 400.
//!   4. GET /v1/theme/logo returns the uploaded bytes with the right
//!      Content-Type and a Cache-Control header.
//!   5. DELETE clears the logo; subsequent GET → 404.

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
        decay_check_interval_secs: 0,
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
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24">
  <rect width="24" height="24" fill="#2563eb"/>
  <text x="12" y="14" font-size="6" text-anchor="middle" fill="#fff">SP</text>
</svg>"##
        .to_vec()
}

fn malicious_svg() -> Vec<u8> {
    br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script><rect width="1" height="1"/></svg>"#
        .to_vec()
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
async fn upload_get_delete_roundtrip() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 1. Upload a valid SVG → 200.
    let svg = good_svg();
    let resp = upload_logo(&c, &h, svg.clone(), "image/svg+xml", "logo.svg").await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "valid SVG upload failed: {body}");
    // The returned theme should reflect the brand name (defaulted to tenant
    // slug on first insert) and have the defaults for everything else.
    assert_eq!(body["brand_name"], "acme");

    // 2. GET /v1/theme/logo returns the bytes with correct headers.
    let resp = c
        .get(format!("{}/v1/theme/logo", h.base))
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
    // Bytes are sanitized — they may differ in whitespace/comments from the
    // original. Spot-check the meaningful content survives.
    let s = String::from_utf8_lossy(&returned);
    assert!(s.contains("<svg"), "logo body should contain <svg: {s}");
    assert!(s.contains("rect"), "logo body should retain shapes: {s}");

    // 3. DELETE → 204; subsequent GET → 404.
    let resp = c
        .delete(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204, "DELETE should return 204");

    let resp = c
        .get(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        404,
        "GET after delete should be 404"
    );

    Ok(())
}

#[tokio::test]
async fn malicious_svg_is_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = upload_logo(&c, &h, malicious_svg(), "image/svg+xml", "evil.svg").await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(
        status, 400,
        "malicious SVG should be rejected, got {status}: {body}"
    );
    let msg = body["message"].as_str().unwrap_or("");
    assert!(
        msg.to_ascii_lowercase().contains("script"),
        "error should mention script: {msg}"
    );

    // No logo should be stored — GET returns 404.
    let resp = c
        .get(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}

#[tokio::test]
async fn oversized_upload_is_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 300 KB of zeros — well over the 256 KB cap. We claim PNG so the
    // sanitizer doesn't reject for content-type before size; the size
    // check fires first anyway.
    let big = vec![0u8; 300 * 1024];
    let resp = upload_logo(&c, &h, big, "image/png", "huge.png").await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 400, "oversized upload should be 400: {body}");
    let msg = body["message"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(
        msg.contains("large") || msg.contains("size"),
        "error should mention size: {msg}"
    );

    Ok(())
}

#[tokio::test]
async fn get_without_upload_returns_404() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = c
        .get(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);
    Ok(())
}

#[tokio::test]
async fn upload_without_auth_is_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // No Authorization header → 401 (the AuthedCaller extractor rejects).
    let part = Part::bytes(good_svg())
        .file_name("logo.svg")
        .mime_str("image/svg+xml")?;
    let form = Form::new().part("file", part);
    let resp = c
        .post(format!("{}/v1/theme/logo", h.base))
        .header("x-skill-pool-tenant", "acme")
        .multipart(form)
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        401,
        "no-token upload should be 401"
    );

    Ok(())
}
