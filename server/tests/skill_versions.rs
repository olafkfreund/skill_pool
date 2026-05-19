//! Integration test: GET /v1/skills/{slug}/versions.
//!
//! Publishes three versions of the same slug and asserts the endpoint
//! returns them in version-desc (created_at-desc) order with the right
//! shape (version, published_at, change_summary, status; published_by
//! omitted when NULL).
//!
//! Mirrors the testcontainers harness used by `skill_detail.rs` so the
//! whole suite stays consistent. Skipped silently when Docker isn't
//! available locally — this slice is opt-in for CI.

use std::net::SocketAddr;
use std::sync::Arc;
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

use skill_pool_server::embedding::{Embedder, SharedEmbedder};
use skill_pool_server::{config, routes, state};

/// No-op embedder — same shape as the dedup harness. Returns a fixed
/// non-zero vector for any input so the publish path is happy.
struct NullEmbedder;
impl Embedder for NullEmbedder {
    fn embed(&self, _text: &str) -> anyhow::Result<Option<Vec<f32>>> {
        let mut v = vec![0.0_f32; 384];
        v[0] = 1.0;
        Ok(Some(v))
    }
    fn dimension(&self) -> Option<usize> {
        Some(384)
    }
}

struct Harness {
    base: String,
    acme_token: String,
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
    let acme_token = admin::create_token(
        &pool,
        "acme",
        "test",
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
    let embedder: SharedEmbedder = Arc::new(NullEmbedder);
    let state = state::AppState::new_with_embedder(&cfg, embedder).await?;
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
fn req(c: &reqwest::Client, m: reqwest::Method, b: &str, p: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{b}{p}"))
        .header("x-skill-pool-tenant", "acme")
}
fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

async fn publish_version(
    c: &reqwest::Client,
    h: &Harness,
    slug: &str,
    version: &str,
    description: &str,
) -> Result<()> {
    let body = format!(
        "---\nname: {slug}\ndescription: {description}\ntags: [test]\n---\n\n# {slug}\n"
    );
    let bundle = build_bundle(&body);
    let meta = json!({ "slug": slug, "version": version });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let r = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/skills"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 201, "{}", r.text().await?);
    Ok(())
}

#[tokio::test]
async fn versions_returns_all_in_desc_order() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // Publish three versions back-to-back. Tiny sleep between each so
    // created_at definitely differs (Postgres timestamps are µs-precise
    // but a same-millisecond burst can tie on slow CI).
    publish_version(&c, &h, "axum-router", "1.0.0", "first cut").await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    publish_version(&c, &h, "axum-router", "1.1.0", "second cut").await?;
    tokio::time::sleep(Duration::from_millis(10)).await;
    publish_version(&c, &h, "axum-router", "2.0.0", "rewrite for axum 0.8").await?;

    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/axum-router/versions"),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await?;
    let arr = body.as_array().unwrap();
    assert_eq!(arr.len(), 3, "expected 3 versions, got {arr:?}");

    // Newest first.
    assert_eq!(arr[0]["version"], "2.0.0");
    assert_eq!(arr[1]["version"], "1.1.0");
    assert_eq!(arr[2]["version"], "1.0.0");

    // Shape.
    for row in arr {
        assert!(row["published_at"].is_string(), "{row}");
        assert!(row["change_summary"].is_string(), "{row}");
        assert_eq!(row["status"], "published");
        // published_by is omitted on null — Phase 1 stores NULL.
        assert!(row.get("published_by").is_none(), "{row}");
    }
    // change_summary mirrors the description.
    assert_eq!(arr[2]["change_summary"], "first cut");
    assert_eq!(arr[0]["change_summary"], "rewrite for axum 0.8");

    // 404 for unknown slug.
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/nope/versions"),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

#[tokio::test]
async fn versions_truncates_long_descriptions_to_200_chars() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let long = "a".repeat(300);
    publish_version(&c, &h, "long-desc", "1.0.0", &long).await?;

    let body: Value = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/long-desc/versions"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    let summary = body[0]["change_summary"].as_str().unwrap();
    // 200 'a' + one ellipsis.
    assert_eq!(summary.chars().count(), 201, "got {summary}");
    assert!(summary.ends_with('…'), "got {summary}");

    Ok(())
}
