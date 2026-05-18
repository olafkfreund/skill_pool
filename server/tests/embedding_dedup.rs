//! Phase 5 integration test: embedding-based dedup of drafts.
//!
//! Uses a deterministic stub `Embedder` so there's no ML dependency, no
//! HuggingFace download, no GPU/CPU embedding cost — pure unit-vector
//! arithmetic over a small set of keyword seeds.
//!
//! Coverage:
//!   1. Publish skill A → POST near-duplicate draft → response carries
//!      `merge_proposal_slug` pointing at A and similarity ≥ 0.85.
//!   2. POST an unrelated draft → no merge_proposal_* fields in response.
//!   3. Tenant isolation: a near-duplicate in another tenant doesn't flag
//!      cross-tenant.

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

/// Deterministic 384-dim embedder keyed off a tiny vocabulary. Two inputs
/// that share a "seed" word get identical vectors → cosine sim = 1.0.
/// Inputs with disjoint seeds get orthogonal vectors → cosine sim = 0.0.
///
/// The seed list maps to distinct dimensions; bias each vector toward its
/// seed's dim with a 1.0 spike, leaving the rest at 0.0. Then normalise.
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
        // If no seed matched, sprinkle a tiny constant into the unused tail
        // so the vector is never the zero vector (which has undefined cosine).
        if v.iter().all(|&x| x == 0.0) {
            v[seeds.len()] = 1.0;
        }
        // L2-normalise so cosine == dot product. Two identical-seed inputs
        // share exact unit vectors → cosine = 1.0.
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
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
    };

    // KEY DIFFERENCE FROM THE OTHER INTEGRATION TESTS: we inject the
    // StubEmbedder so dedup actually does something. The Config still
    // says embedding.enabled=false because the stub bypasses that switch.
    let embedder: SharedEmbedder = Arc::new(StubEmbedder);
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
) -> Result<Value> {
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: {description}\ntags: [seeded]\n---\n\n# {slug}\n"
    ));
    let meta = json!({ "slug": slug, "version": "1.0.0" });
    let form = Form::new()
        .text("metadata", meta.to_string())
        .part(
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
    let body: Value = resp.json().await?;
    assert_eq!(status, 201, "publish failed: {body}");
    Ok(body)
}

async fn create_draft(
    c: &reqwest::Client,
    h: &Harness,
    tenant: &str,
    token: &str,
    slug: &str,
    description: &str,
) -> Result<Value> {
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: {description}\ntags: [seeded]\n---\n\n# {slug}\n"
    ));
    let meta = json!({ "slug": slug, "origin": "cli" });
    let form = Form::new()
        .text("metadata", meta.to_string())
        .part(
            "bundle",
            Part::bytes(bundle.to_vec())
                .file_name(format!("{slug}.tar.gz"))
                .mime_str("application/gzip")?,
        );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/drafts", tenant),
        token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 201, "draft create failed: {body}");
    Ok(body)
}

#[tokio::test]
async fn dedup_flags_near_duplicate_drafts() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // 1. Publish an existing skill in acme with a seed word.
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "axum-handler",
        "Pattern for axum tenant-scoped extractors",
    )
    .await?;

    // 2. Submit a near-duplicate draft in acme (same seed → cosine = 1.0).
    let dup_draft = create_draft(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "another-axum-tip",
        "Tip about axum middleware composition",
    )
    .await?;

    assert_eq!(
        dup_draft["merge_proposal_slug"], "axum-handler",
        "expected merge_proposal_slug=axum-handler, got {dup_draft}"
    );
    let sim = dup_draft["merge_proposal_similarity"]
        .as_f64()
        .expect("merge_proposal_similarity should be a number");
    assert!(
        sim >= 0.85,
        "expected similarity ≥ 0.85, got {sim} for {dup_draft}"
    );

    // 3. Submit an UNrelated draft (different seed → cosine ≈ 0).
    let unrelated_draft = create_draft(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "kafka-consumer-tip",
        "How to write a kafka consumer with backpressure",
    )
    .await?;
    assert!(
        unrelated_draft.get("merge_proposal_slug").is_none()
            || unrelated_draft["merge_proposal_slug"].is_null(),
        "unrelated draft should NOT carry a merge proposal: {unrelated_draft}"
    );

    // 4. Cross-tenant isolation: globex publishes a draft with the same seed
    //    as acme's published skill — must NOT be flagged because dedup is
    //    scoped per tenant.
    let cross_tenant = create_draft(
        &c,
        &h,
        "globex",
        &h.globex_token,
        "axum-in-globex",
        "Pattern for axum in a different tenant",
    )
    .await?;
    assert!(
        cross_tenant.get("merge_proposal_slug").is_none()
            || cross_tenant["merge_proposal_slug"].is_null(),
        "cross-tenant draft should NOT flag: {cross_tenant}"
    );

    // 5. The flag survives GET /v1/drafts (the JOIN works in the list path).
    let list: Vec<Value> = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/drafts", "acme"),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;
    let flagged = list
        .iter()
        .find(|d| d["slug"] == "another-axum-tip")
        .expect("draft missing from list");
    assert_eq!(flagged["merge_proposal_slug"], "axum-handler");

    Ok(())
}

#[tokio::test]
async fn dedup_is_noop_when_no_existing_skills() -> Result<()> {
    // Fresh tenant, no skills published. A new draft must NOT be flagged
    // (there's nothing to flag against) even though the embedder is wired.
    let h = boot().await?;
    let c = client();

    let draft = create_draft(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "lonely-draft",
        "Something about axum",
    )
    .await?;
    assert!(
        draft.get("merge_proposal_slug").is_none() || draft["merge_proposal_slug"].is_null(),
        "no existing skill → no merge proposal possible: {draft}"
    );
    Ok(())
}
