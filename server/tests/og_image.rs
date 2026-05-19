//! Integration test for `GET /v1/og` — the per-tenant Open Graph image
//! generator (#9).
//!
//! Covers the full contract from the spec:
//!   1. Publish a skill, set a theme, GET → 200 with correct
//!      Content-Type, Cache-Control, ETag, and a non-empty SVG body.
//!   2. A second GET returns the same ETag (renderer is deterministic).
//!   3. `If-None-Match` echo → 304.
//!   4. Unknown slug → 404 (we document this in `og-images.md`).
//!   5. Missing slug → 400.
//!   6. Unknown kind → 400.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

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
    let port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    let token = admin::create_token(
        &pool,
        "acme",
        "tester",
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
        token,
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

fn build_bundle(skill_md: &str) -> Bytes {
    let mut tar = tar::Builder::new(Vec::new());
    let body = skill_md.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_path("SKILL.md").unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, body).unwrap();
    let tar_bytes = tar.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    Bytes::from(gz.finish().unwrap())
}

async fn publish(c: &reqwest::Client, h: &Harness, slug: &str) -> Result<()> {
    let body = format!(
        "---\nname: {slug}\ndescription: A practical recipe for {slug} that explains the why and the how clearly enough to skim.\ntags: [test]\n---\n\n# {slug}\n"
    );
    let bundle = build_bundle(&body);
    let meta = json!({ "slug": slug, "version": "1.2.3" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let r = c
        .post(format!("{}/v1/skills", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.token)
        .multipart(form)
        .send()
        .await?;
    let status = r.status().as_u16();
    let body = r.text().await?;
    assert_eq!(status, 201, "publish failed: {body}");
    Ok(())
}

async fn put_theme(c: &reqwest::Client, h: &Harness) -> Result<()> {
    let body = json!({
        "brand_name": "Acme Corp",
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
    let r = c
        .put(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.token)
        .json(&body)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200, "theme put failed: {}", r.text().await?);
    Ok(())
}

#[tokio::test]
async fn og_image_basic_render_etag_and_304() -> Result<()> {
    let h = boot().await?;
    let c = client();

    put_theme(&c, &h).await?;
    publish(&c, &h, "axum-handler").await?;

    // 1. Initial GET → 200 + headers + non-empty SVG.
    let r = c
        .get(format!("{}/v1/og?slug=axum-handler", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200);
    let ct = r
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert!(
        ct.starts_with("image/svg+xml"),
        "unexpected content-type: {ct}"
    );
    let cache = r
        .headers()
        .get("cache-control")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(cache, "public, max-age=86400");
    let etag = r
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .expect("missing ETag");
    assert!(etag.starts_with("\"og-"), "unexpected etag shape: {etag}");
    let bytes = r.bytes().await?;
    assert!(!bytes.is_empty(), "expected non-empty body");
    let s = String::from_utf8_lossy(&bytes);
    assert!(s.contains("<svg"), "body should be SVG: {}", &s[..120.min(s.len())]);
    assert!(s.contains("axum-handler"), "should embed slug");
    assert!(s.contains("v1.2.3"), "should embed version pill");
    assert!(s.contains("Acme Corp"), "should embed brand name");

    // 2. Second GET → identical ETag (deterministic for unchanged inputs).
    let r2 = c
        .get(format!("{}/v1/og?slug=axum-handler", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r2.status().as_u16(), 200);
    let etag2 = r2
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(etag2, etag, "ETag should be stable across calls");

    // 3. If-None-Match echo → 304 with no body, headers still set.
    let r3 = c
        .get(format!("{}/v1/og?slug=axum-handler", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("if-none-match", &etag)
        .send()
        .await?;
    assert_eq!(r3.status().as_u16(), 304);
    let etag3 = r3
        .headers()
        .get("etag")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(etag3, etag, "304 must echo the same ETag");

    Ok(())
}

#[tokio::test]
async fn og_image_unknown_slug_is_404() -> Result<()> {
    let h = boot().await?;
    let c = client();
    put_theme(&c, &h).await?;
    // No publish — slug doesn't exist.
    let r = c
        .get(format!("{}/v1/og?slug=does-not-exist", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 404);
    Ok(())
}

#[tokio::test]
async fn og_image_missing_slug_is_400() -> Result<()> {
    let h = boot().await?;
    let c = client();
    // No slug param at all → 400.
    let r = c
        .get(format!("{}/v1/og", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 400);
    // Empty slug also 400 (trimmed empty string).
    let r2 = c
        .get(format!("{}/v1/og?slug=", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r2.status().as_u16(), 400);
    Ok(())
}

#[tokio::test]
async fn og_image_unknown_kind_is_400() -> Result<()> {
    let h = boot().await?;
    let c = client();
    let r = c
        .get(format!("{}/v1/og?slug=foo&kind=hammer", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 400);
    Ok(())
}

#[tokio::test]
async fn og_image_works_without_explicit_theme() -> Result<()> {
    // Tenant hasn't called PUT /v1/theme yet — we should still render
    // using the default theme rather than 500.
    let h = boot().await?;
    let c = client();
    publish(&c, &h, "axum-handler").await?;

    let r = c
        .get(format!("{}/v1/og?slug=axum-handler", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body = r.text().await?;
    assert!(body.contains("<svg"), "expected SVG body");
    // The defaulted brand_name is the tenant slug.
    assert!(body.contains("acme"), "default brand should be slug `acme`");
    Ok(())
}
