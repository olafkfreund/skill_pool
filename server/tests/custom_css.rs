//! Integration test for the per-tenant custom-CSS pipeline (issue #9, L28).
//!
//! Covers:
//!   1. Valid CSS upload → 200, theme row updated.
//!   2. GET /v1/theme/custom.css → 200 with body, correct Content-Type,
//!      Cache-Control, and Content-Security-Policy headers.
//!   3. Sanitizer rejections — @import, off-site url(), comment-hidden @import.
//!   4. Oversized payload (>32 KiB) → 400.
//!   5. DELETE → 204; subsequent GET → 404.

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

async fn upload(
    c: &reqwest::Client,
    h: &Harness,
    body: Vec<u8>,
) -> Result<reqwest::Response> {
    let part = Part::bytes(body)
        .file_name("overlay.css")
        .mime_str("text/css")?;
    let form = Form::new().part("file", part);
    let resp = c
        .post(format!("{}/v1/theme/custom-css", h.base))
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

    let css = b".sp-hero { color: #336699; background: var(--sp-primary); }".to_vec();
    let resp = upload(&c, &h, css.clone()).await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "valid CSS upload failed: {body}");

    // GET returns bytes + headers.
    let resp = c
        .get(format!("{}/v1/theme/custom.css", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/css; charset=utf-8"),
    );
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=300"),
    );
    assert_eq!(
        resp.headers()
            .get("content-security-policy")
            .and_then(|v| v.to_str().ok()),
        Some("style-src 'self'"),
    );
    assert_eq!(
        resp.headers()
            .get("x-content-type-options")
            .and_then(|v| v.to_str().ok()),
        Some("nosniff"),
    );
    let returned = resp.bytes().await?;
    assert_eq!(returned.as_ref(), css.as_slice(), "body should match upload byte-for-byte");

    // DELETE → 204.
    let resp = c
        .delete(format!("{}/v1/theme/custom-css", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204);

    // Subsequent GET → 404.
    let resp = c
        .get(format!("{}/v1/theme/custom.css", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}

#[tokio::test]
async fn rejects_import() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let css = br#"@import url("https://evil.com/x.css");"#.to_vec();
    let resp = upload(&c, &h, css).await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 400, "@import upload should be 400: {body}");
    let msg = body["message"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(msg.contains("@import"), "error should mention @import: {msg}");

    Ok(())
}

#[tokio::test]
async fn rejects_external_url() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let css = br#".sp-hero { background: url(https://evil.com/x.png); }"#.to_vec();
    let resp = upload(&c, &h, css).await?;
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().await?;
    let msg = body["message"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(
        msg.contains("url"),
        "error should mention url(): {msg}",
    );

    Ok(())
}

#[tokio::test]
async fn rejects_comment_hidden_import() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // Comment-hidden @import. The naive scan misses it; the sanitizer's
    // strip-then-rescan catches it.
    let css = br#"/* harmless */@import url(evil);"#.to_vec();
    let resp = upload(&c, &h, css).await?;
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().await?;
    let msg = body["message"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(msg.contains("@import"), "error should mention @import: {msg}");

    Ok(())
}

#[tokio::test]
async fn rejects_oversized() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 40 KiB — over the 32 KiB cap.
    let big = vec![b'a'; 40 * 1024];
    let resp = upload(&c, &h, big).await?;
    assert_eq!(resp.status().as_u16(), 400);
    let body: Value = resp.json().await?;
    let msg = body["message"].as_str().unwrap_or("").to_ascii_lowercase();
    assert!(
        msg.contains("large") || msg.contains("size"),
        "error should mention size: {msg}"
    );

    Ok(())
}

#[tokio::test]
async fn upload_without_auth_is_rejected() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let part = Part::bytes(b".x { color: red; }".to_vec())
        .file_name("overlay.css")
        .mime_str("text/css")?;
    let form = Form::new().part("file", part);
    let resp = c
        .post(format!("{}/v1/theme/custom-css", h.base))
        .header("x-skill-pool-tenant", "acme")
        .multipart(form)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 401);

    Ok(())
}

#[tokio::test]
async fn get_without_upload_returns_404() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let resp = c
        .get(format!("{}/v1/theme/custom.css", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}
