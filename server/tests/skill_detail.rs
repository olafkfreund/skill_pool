//! Phase 5 integration test: GET /v1/skills/{slug}/detail.
//!
//! Confirms the detail endpoint returns base metadata + use_count +
//! last_used_at + forward deps (with target versions) + reverse deps
//! (required_by) + pending merge proposals, in one round trip.

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

/// Tiny stub embedder so we can exercise the merge_proposal path without
/// any ML dependency. Same seeded-vector pattern as the embedding_dedup
/// test: same keyword → identical vector → cosine 1.0.
struct StubEmbedder;
impl Embedder for StubEmbedder {
    fn embed(&self, text: &str) -> anyhow::Result<Option<Vec<f32>>> {
        let lc = text.to_lowercase();
        let seeds = ["axum", "kafka", "react"];
        let mut v = vec![0.0_f32; 384];
        for (i, s) in seeds.iter().enumerate() {
            if lc.contains(s) {
                v[i] = 1.0;
            }
        }
        if v.iter().all(|&x| x == 0.0) {
            v[seeds.len()] = 1.0;
        }
        let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        if n > 0.0 {
            for x in &mut v {
                *x /= n;
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

async fn publish(c: &reqwest::Client, h: &Harness, slug: &str, requires: &[&str]) -> Result<()> {
    let req_block = if requires.is_empty() {
        String::new()
    } else {
        format!(
            "requires:\n{}\n",
            requires
                .iter()
                .map(|r| format!("  - {r}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    let body = format!(
        "---\nname: {slug}\ndescription: Pattern about {slug}.\ntags: [test]\n{req_block}---\n\n# {slug}\n"
    );
    let bundle = build_bundle(&body);
    let meta = json!({ "slug": slug, "version": "1.0.0" });
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

async fn create_draft(c: &reqwest::Client, h: &Harness, slug: &str) -> Result<()> {
    let body = format!(
        "---\nname: {slug}\ndescription: A new axum draft for testing.\ntags: [draft]\n---\n\n# {slug}\n"
    );
    let bundle = build_bundle(&body);
    let meta = json!({ "slug": slug, "origin": "cli" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let r = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/drafts"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 201, "{}", r.text().await?);
    Ok(())
}

#[tokio::test]
async fn skill_detail_returns_full_view() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // Catalog: axum-handler (requires nothing) ← parent we'll inspect.
    // Two siblings that require it.
    publish(&c, &h, "axum-handler", &[]).await?;
    publish(&c, &h, "axum-middleware", &["axum-handler"]).await?;
    publish(&c, &h, "axum-tenant-ext", &["axum-handler@1.0.0"]).await?;

    // axum-handler itself requires nothing yet — publish a v2 that does.
    // Schema-wise we just publish a new version; the detail query reads
    // the latest version per slug.
    let body = "---\nname: axum-handler\ndescription: v2 with deps.\nrequires:\n  - tower-layer\n---\n\n# axum-handler\n";
    let bundle = build_bundle(body);
    let meta = json!({ "slug": "axum-handler", "version": "2.0.0" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name("axum-handler.tar.gz")
            .mime_str("application/gzip")?,
    );
    let r = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/skills"),
        &h.acme_token,
    )
    .multipart(form)
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 201);

    // Bundle download bumps use_count.
    for _ in 0..3 {
        let r = authed(
            req(
                &c,
                reqwest::Method::GET,
                &h.base,
                "/v1/skills/axum-handler/bundle.tar.gz",
            ),
            &h.acme_token,
        )
        .send()
        .await?;
        assert_eq!(r.status().as_u16(), 200);
    }

    // Capture a draft that the embedding dedup will mark as a merge
    // proposal pointing at axum-handler (same `axum` seed).
    create_draft(&c, &h, "axum-pattern-tip").await?;

    // ---- The detail endpoint ----
    let body: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/skills/axum-handler/detail",
        ),
        &h.acme_token,
    )
    .send()
    .await?
    .json()
    .await?;

    // Base fields reflect the latest version (v2.0.0 with deps).
    assert_eq!(body["slug"], "axum-handler");
    assert_eq!(body["version"], "2.0.0");
    assert_eq!(body["status"], "published");
    // use_count from the bundle downloads.
    let uc = body["use_count"].as_i64().unwrap();
    assert!(
        uc >= 1,
        "expected use_count >= 1 (only the v2 row counts here), got {uc}"
    );
    assert!(body["last_used_at"].is_string(), "{body}");

    // Forward deps: v2 declares tower-layer. tower-layer is unpublished
    // so its version surfaces as "".
    let requires = body["requires"].as_array().unwrap();
    assert_eq!(requires.len(), 1, "{requires:?}");
    assert_eq!(requires[0]["slug"], "tower-layer");
    assert_eq!(requires[0]["version"], "");
    assert_eq!(requires[0]["version_range"], "*");

    // Reverse deps: two published skills depend on axum-handler.
    let required_by = body["required_by"].as_array().unwrap();
    let slugs: Vec<&str> = required_by
        .iter()
        .map(|r| r["slug"].as_str().unwrap())
        .collect();
    assert!(slugs.contains(&"axum-middleware"), "{required_by:?}");
    assert!(slugs.contains(&"axum-tenant-ext"), "{required_by:?}");
    let ext = required_by
        .iter()
        .find(|r| r["slug"] == "axum-tenant-ext")
        .unwrap();
    assert_eq!(ext["version_range"], "1.0.0");
    assert_eq!(ext["version"], "1.0.0");

    // Pending merge proposal: the seeded draft.
    let proposals = body["merge_proposals"].as_array().unwrap();
    assert_eq!(proposals.len(), 1, "{proposals:?}");
    assert_eq!(proposals[0]["draft_slug"], "axum-pattern-tip");
    let sim = proposals[0]["similarity"].as_f64().unwrap();
    assert!(sim >= 0.85, "expected similarity ≥ 0.85, got {sim}");

    // 404 on unknown slug.
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/skills/nope/detail"),
        &h.acme_token,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}
