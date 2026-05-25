//! Phase 5 integration test: semantic search via `GET /v1/skills?semantic=…`.
//!
//! Reuses the same deterministic StubEmbedder pattern as the dedup test
//! — same input keywords produce identical 384-dim unit vectors, so two
//! skills with overlapping seed words are cosine-1.0 to each other and
//! orthogonal to anything else.
//!
//! Coverage:
//!  1. semantic=foo returns the matching skill first with similarity ≥ 0.85,
//!     ranked above unrelated skills, with the `similarity` field populated.
//!  2. min_similarity filters out the long tail.
//!  3. Tags AND semantic together — both filters apply.
//!  4. Tenant isolation — globex's matching skill never leaks into acme's results.
//!  5. NullEmbedder server → 400 with the configured message.
//!  6. No semantic param → existing keyword/tag behavior unchanged, no
//!     `similarity` field in the response.

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

use skill_pool_server::embedding::{Embedder, NullEmbedder, SharedEmbedder};
use skill_pool_server::{config, routes, state};

/// Same shape as in `embedding_dedup.rs` — kept inline rather than
/// extracted to `tests/common/mod.rs` because the duplication is small
/// and shared test modules complicate the integration-test layout.
struct StubEmbedder;

impl Embedder for StubEmbedder {
    fn embed(&self, text: &str) -> anyhow::Result<Option<Vec<f32>>> {
        let lc = text.to_lowercase();
        let seeds = [
            "axum", "react", "kafka", "postgres", "graphql", "rust", "python", "tailwind",
        ];
        let dim = 384;
        let mut v = vec![0.0_f32; dim];
        for (i, seed) in seeds.iter().enumerate() {
            if lc.contains(seed) {
                v[i] = 1.0;
            }
        }
        if v.iter().all(|&x| x == 0.0) {
            v[seeds.len()] = 1.0;
        }
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for x in &mut v {
                *x /= norm;
            }
        }
        Ok(Some(v))
    }
    fn dimension(&self) -> Option<usize> {
        Some(384)
    }
}

struct Harness {
    base: String,
    acme_token: String,
    globex_token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot(embedder: SharedEmbedder) -> Result<Harness> {
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

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex Inc", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;
    let globex_token = admin::create_token(&pool, "globex", "test", "skills:read skills:publish")
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

fn req(
    c: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    path: &str,
    tenant: &str,
) -> reqwest::RequestBuilder {
    c.request(method, format!("{base}{path}"))
        .header("x-skill-pool-tenant", tenant)
}

fn authed(b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(token)
}

async fn publish_skill(
    c: &reqwest::Client,
    h: &Harness,
    tenant: &str,
    token: &str,
    slug: &str,
    description: &str,
    tags: &[&str],
) -> Result<()> {
    let tag_yaml = tags
        .iter()
        .map(|t| format!("- {t}"))
        .collect::<Vec<_>>()
        .join("\n");
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: {description}\ntags:\n{tag_yaml}\n---\n\n# {slug}\n"
    ));
    let meta = json!({ "slug": slug, "version": "1.0.0" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/skills", tenant),
        token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    if status != 201 {
        let body = resp.text().await?;
        anyhow::bail!("publish {slug} failed ({status}): {body}");
    }
    Ok(())
}

#[tokio::test]
async fn semantic_search_ranks_by_similarity_and_returns_similarity_field() -> Result<()> {
    let h = boot(Arc::new(StubEmbedder)).await?;
    let c = client();

    // Populate acme with three published skills covering different seed words.
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "axum-handler",
        "Pattern for axum tenant-scoped extractors",
        &["rust"],
    )
    .await?;
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "react-server-components",
        "React server components for streaming UIs",
        &["frontend"],
    )
    .await?;
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "kafka-consumer",
        "Kafka consumer with backpressure",
        &["streaming"],
    )
    .await?;

    // 1. Semantic search for "axum middleware" returns axum-handler first.
    let url = "/v1/skills?semantic=axum%20middleware%20pattern";
    let results: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, url, "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;

    assert!(!results.is_empty(), "expected results: {results:?}");
    let first = &results[0];
    assert_eq!(first["slug"], "axum-handler", "{results:?}");
    let sim = first["similarity"]
        .as_f64()
        .expect("similarity should be present and numeric");
    assert!(sim >= 0.85, "similarity = {sim}, expected ≥ 0.85");

    // 2. min_similarity filters everything below.
    let url = "/v1/skills?semantic=axum&min_similarity=0.9";
    let filtered: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, url, "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        filtered.iter().all(|r| r["slug"] == "axum-handler"),
        "min_similarity=0.9 should leave only the axum result, got {filtered:?}"
    );

    // 3. Semantic + tag filter together — tag must match (rust) AND seed.
    let url = "/v1/skills?semantic=axum&tags=rust";
    let combined: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, url, "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(combined.len(), 1);
    assert_eq!(combined[0]["slug"], "axum-handler");

    // 4. Tenant isolation: globex publishes a matching skill — acme's search
    //    still returns ONLY acme's results.
    publish_skill(
        &c,
        &h,
        "globex",
        &h.globex_token,
        "globex-axum-tip",
        "axum tip in another tenant",
        &[],
    )
    .await?;
    let url = "/v1/skills?semantic=axum";
    let acme_only: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, url, "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(
        acme_only.iter().all(|r| r["slug"] != "globex-axum-tip"),
        "cross-tenant leak: {acme_only:?}"
    );
    assert!(
        acme_only.iter().any(|r| r["slug"] == "axum-handler"),
        "acme's axum-handler missing: {acme_only:?}"
    );

    // 5. NO semantic param → existing keyword behavior, no similarity field.
    let plain: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills", "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(plain.len() >= 3);
    for r in &plain {
        assert!(
            r.get("similarity").is_none() || r["similarity"].is_null(),
            "plain list must not carry similarity: {r}"
        );
    }

    Ok(())
}

#[tokio::test]
async fn semantic_search_rejects_when_no_embedder_configured() -> Result<()> {
    // NullEmbedder simulates a default-build server without --features fastembed.
    let h = boot(Arc::new(NullEmbedder)).await?;
    let c = client();

    let url = "/v1/skills?semantic=anything";
    let resp = authed(
        req(&c, reqwest::Method::GET, &h.base, url, "acme"),
        &h.acme_token,
    )
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 400, "{body}");
    let msg = body["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("semantic search is not enabled"),
        "unexpected message: {body}"
    );
    Ok(())
}
