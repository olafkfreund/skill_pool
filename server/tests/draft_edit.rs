//! Integration test: editable drafts (PATCH /v1/drafts/{id}).
//!
//! Covers:
//!   1. Happy path — change slug, description, when_to_use, tags, notes.
//!   2. Partial — only the fields present in the body are touched.
//!   3. Clear nullable column via empty string.
//!   4. Empty slug / description → 400.
//!   5. Already-published draft → 400.
//!   6. Cross-tenant draft → 404.
//!   7. Unknown id → 404.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

struct Harness {
    base: String,
    acme_token: String,
    globex_token: String,
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

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;
    let globex_token =
        admin::create_token(&pool, "globex", "test", "skills:read skills:publish")
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
    let state = state::AppState::new(&cfg).await?;
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        acme_token,
        globex_token,
        _pg: pg,
        _storage_dir: storage_dir,
    })
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

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn req(c: &reqwest::Client, m: reqwest::Method, base: &str, p: &str, t: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{base}{p}")).header("x-skill-pool-tenant", t)
}
fn authed(b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(token)
}

async fn create_pending_draft(c: &reqwest::Client, h: &Harness) -> Result<String> {
    let bundle = build_bundle(
        "---\nname: original\ndescription: original description.\nwhen_to_use: when something happens.\ntags: [original]\n---\n\n# original\n",
    );
    let meta = json!({ "slug": "original", "origin": "cli", "notes": "original note" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name("original.tar.gz")
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/drafts", "acme"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 201, "{body}");
    Ok(body["id"].as_str().unwrap().to_string())
}

#[tokio::test]
async fn patch_draft_round_trip() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 1. Happy path — change every field.
    let id = create_pending_draft(&c, &h).await?;
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            &format!("/v1/drafts/{id}"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({
        "slug": "renamed-slug",
        "description": "rewritten description.",
        "when_to_use": "when X precondition holds.",
        "tags": ["edited", "rust"],
        "notes": "Updated by curator."
    }))
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["slug"], "renamed-slug");
    assert_eq!(body["description"], "rewritten description.");
    assert_eq!(body["when_to_use"], "when X precondition holds.");
    let tags = body["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 2);
    assert_eq!(body["notes"], "Updated by curator.");

    // 2. Partial — change only slug.
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            &format!("/v1/drafts/{id}"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "slug": "renamed-again" }))
    .send()
    .await?;
    let body: Value = resp.json().await?;
    assert_eq!(body["slug"], "renamed-again");
    // Other fields preserved from step 1.
    assert_eq!(body["description"], "rewritten description.");
    let tags = body["tags"].as_array().unwrap();
    assert_eq!(tags.len(), 2);

    // 3. Clear nullable column via empty string.
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            &format!("/v1/drafts/{id}"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "when_to_use": "", "notes": "" }))
    .send()
    .await?;
    let body: Value = resp.json().await?;
    assert!(
        body.get("when_to_use").is_none_or(|v| v.is_null()),
        "when_to_use should be cleared: {body}"
    );
    assert!(
        body.get("notes").is_none_or(|v| v.is_null()),
        "notes should be cleared: {body}"
    );

    // 4. Empty slug → 400.
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            &format!("/v1/drafts/{id}"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "slug": "   " }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 400, "{}", resp.text().await?);

    // 5. Cross-tenant draft → 404 (globex can't see acme's draft).
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            &format!("/v1/drafts/{id}"),
            "globex",
        ),
        &h.globex_token,
    )
    .json(&json!({ "slug": "stolen" }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    // 6. Already-published draft → 400.
    let resp = authed(
        req(
            &c,
            reqwest::Method::POST,
            &h.base,
            &format!("/v1/drafts/{id}/publish"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "version": "1.0.0" }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 200, "publish failed: {}", resp.text().await?);
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            &format!("/v1/drafts/{id}"),
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "slug": "too-late" }))
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 400, "{body}");
    assert!(body["message"]
        .as_str()
        .unwrap_or_default()
        .contains("only pending drafts"));

    // 7. Unknown id → 404.
    let resp = authed(
        req(
            &c,
            reqwest::Method::PATCH,
            &h.base,
            "/v1/drafts/00000000-0000-0000-0000-000000000000",
            "acme",
        ),
        &h.acme_token,
    )
    .json(&json!({ "slug": "ghost" }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 404);

    Ok(())
}
