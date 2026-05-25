//! Integration tests for `GET /v1/bootstrap` after the tier-2 + tier-3
//! fallback wiring.
//!
//! Tier 1 (curated mapping) already has unit coverage via the admin
//! `set_stack_mapping` call; these tests focus on the new behaviour:
//!
//!   1. Empty `tenant_stack_mappings` still returns skills via tier 2
//!      (tag intersection) and tier 3 (semantic similarity).
//!   2. A slug surfaced by *both* curated and tag-intersection appears
//!      exactly once and is attributed to `curated` under `?debug=1`.
//!   3. The 8-slug cap is respected even when the union is larger.
//!
//! The harness reuses the StubEmbedder pattern from `semantic_search.rs`
//! / `embedding_dedup.rs` — same deterministic 384-dim seed-keyed
//! vectors, no fastembed download, no GPU/CPU embedding cost.

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
use skill_pool_server::{admin, config, routes, state};

/// Deterministic 384-dim embedder keyed off a tiny vocabulary. Two inputs
/// that share a seed word produce identical unit vectors → cosine sim 1.0.
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
    pool: sqlx::PgPool,
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

    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
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

    let app_state = state::AppState::new_with_embedder(&cfg, embedder).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        acme_token,
        pool,
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

async fn bootstrap_get(
    c: &reqwest::Client,
    h: &Harness,
    tenant: &str,
    token: &str,
    query: &str,
) -> Result<Value> {
    let url = format!("/v1/bootstrap?{query}");
    let body = authed(req(c, reqwest::Method::GET, &h.base, &url, tenant), token)
        .send()
        .await?
        .json()
        .await?;
    Ok(body)
}

/// Tier 2 + tier 3 surface skills when curated is empty.
#[tokio::test]
async fn bootstrap_falls_back_to_tag_and_semantic() -> Result<()> {
    let h = boot(Arc::new(StubEmbedder)).await?;
    let c = client();

    // No curated rows. We have:
    //   - axum-handler: tagged with `rust` (tier 2 hit on stack=rust,axum)
    //                   AND mentions "axum" in description (tier 3 hit too)
    //   - kafka-tip:    tagged with `streaming` only — irrelevant
    //   - tailwind-cookbook: tagged with `frontend` — irrelevant
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
        "kafka-tip",
        "Kafka consumer with backpressure",
        &["streaming"],
    )
    .await?;
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "tailwind-cookbook",
        "Tailwind CSS utility-class recipes",
        &["frontend"],
    )
    .await?;

    // Sanity: no curated mappings yet.
    let body = bootstrap_get(&c, &h, "acme", &h.acme_token, "stack=rust,axum&debug=1").await?;
    let skills: Vec<String> = serde_json::from_value(body["skills"].clone())?;
    assert!(
        skills.contains(&"axum-handler".to_string()),
        "expected axum-handler in fallback results: {body}"
    );
    // debug=1 → tier_breakdown present, curated empty, axum-handler attributed
    // to tier 2 (matched the `rust` tag) — semantic also picked it up but
    // dedup keeps it in the higher tier.
    let tb = &body["tier_breakdown"];
    assert!(
        tb.is_object(),
        "expected tier_breakdown with debug=1: {body}"
    );
    let curated: Vec<String> = serde_json::from_value(tb["curated"].clone())?;
    let tagged: Vec<String> = serde_json::from_value(tb["tagged"].clone())?;
    assert!(curated.is_empty(), "curated should be empty: {tb}");
    assert!(
        tagged.contains(&"axum-handler".to_string()),
        "axum-handler must be attributed to tagged tier: {tb}"
    );

    Ok(())
}

/// A slug present in both curated and tag-intersection appears once in
/// `skills` and is attributed to `curated` in the breakdown.
#[tokio::test]
async fn bootstrap_dedups_curated_over_tag_intersection() -> Result<()> {
    let h = boot(Arc::new(StubEmbedder)).await?;
    let c = client();

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

    // Curate the same slug under the same stack tag.
    admin::set_stack_mapping(&h.pool, "acme", "rust", "axum-handler").await?;

    let body = bootstrap_get(&c, &h, "acme", &h.acme_token, "stack=rust&debug=1").await?;
    let skills: Vec<String> = serde_json::from_value(body["skills"].clone())?;

    // Appears exactly once.
    let count = skills.iter().filter(|s| *s == "axum-handler").count();
    assert_eq!(count, 1, "duplicate slug in response: {skills:?}");

    // Attributed to `curated`, NOT to `tagged`.
    let tb = &body["tier_breakdown"];
    let curated: Vec<String> = serde_json::from_value(tb["curated"].clone())?;
    let tagged: Vec<String> = serde_json::from_value(tb["tagged"].clone())?;
    assert!(
        curated.contains(&"axum-handler".to_string()),
        "expected curated attribution: {tb}"
    );
    assert!(
        !tagged.contains(&"axum-handler".to_string()),
        "should not be attributed to tagged after curated win: {tb}"
    );

    Ok(())
}

/// The hard cap is 8 — even when curated + tagged + semantic union to
/// more than that, the response is truncated.
#[tokio::test]
async fn bootstrap_caps_at_eight_slugs() -> Result<()> {
    let h = boot(Arc::new(StubEmbedder)).await?;
    let c = client();

    // Publish 12 distinct skills, all tagged `rust` so tier 2 finds them.
    // The semantic tier will also rank them, but dedup keeps them in
    // tier 2.
    for i in 0..12 {
        let slug = format!("rust-tip-{i:02}");
        publish_skill(
            &c,
            &h,
            "acme",
            &h.acme_token,
            &slug,
            &format!("Rust tip number {i} about axum and tokio"),
            &["rust"],
        )
        .await?;
    }

    let body = bootstrap_get(&c, &h, "acme", &h.acme_token, "stack=rust").await?;
    let skills: Vec<String> = serde_json::from_value(body["skills"].clone())?;
    assert_eq!(
        skills.len(),
        8,
        "expected cap=8, got {} ({skills:?})",
        skills.len()
    );

    // No tier_breakdown when debug is off.
    assert!(
        body.get("tier_breakdown").is_none_or(|v| v.is_null()),
        "tier_breakdown leaked into non-debug response: {body}"
    );

    Ok(())
}

/// `?project=acme-billing&stack=rust` — project items (tier 0) appear first,
/// then rust-stack-mapping items backfill remaining slots up to 8.
#[tokio::test]
async fn bootstrap_project_tier0_precedes_stack_tiers() -> Result<()> {
    let h = boot(Arc::new(StubEmbedder)).await?;
    let c = client();

    // Admin token (needs tenant:admin to create the project).
    let admin_token = admin::create_token(
        &h.pool,
        "acme",
        "admin-tok",
        "tenant:admin skills:read skills:publish",
    )
    .await?
    .raw_token;

    // Create a project named "acme-billing" and give it 3 items.
    let proj_body = serde_json::json!({
        "slug": "acme-billing",
        "name": "Acme Billing Service"
    });
    let r = c
        .post(format!("{}/v1/tenant/projects", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_token)
        .json(&proj_body)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 201, "{}", r.text().await?);

    let items_body = serde_json::json!([
        {"slug": "billing-migrations", "kind": "skill"},
        {"slug": "billing-agent",      "kind": "agent"},
        {"slug": "billing-cmd",        "kind": "command"}
    ]);
    let r = c
        .put(format!("{}/v1/tenant/projects/acme-billing/items", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&admin_token)
        .json(&items_body)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 204);

    // Publish a skill tagged `rust` so tier 1 (curated mapping) can be set.
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "axum-handler",
        "Pattern for axum handlers",
        &["rust"],
    )
    .await?;
    // Add a curated stack mapping for rust → axum-handler.
    admin::set_stack_mapping(&h.pool, "acme", "rust", "axum-handler").await?;

    // Bootstrap with project + stack.
    let body = bootstrap_get(
        &c,
        &h,
        "acme",
        &admin_token,
        "project=acme-billing&stack=rust&debug=1",
    )
    .await?;

    let skills: Vec<String> = serde_json::from_value(body["skills"].clone())?;
    // Project items come first.
    assert_eq!(
        skills[0], "billing-migrations",
        "first skill must be first project item: {skills:?}"
    );
    assert_eq!(skills[1], "billing-agent");
    assert_eq!(skills[2], "billing-cmd");
    // Stack-mapping item follows.
    assert!(
        skills.contains(&"axum-handler".to_string()),
        "axum-handler (curated) must appear after project items: {skills:?}"
    );

    // Response carries `project` field.
    assert_eq!(body["project"]["slug"], "acme-billing");
    assert_eq!(body["project"]["name"], "Acme Billing Service");

    // Debug tier_breakdown shows project items in `project` bucket.
    let tb = &body["tier_breakdown"];
    let proj_tier: Vec<String> = serde_json::from_value(tb["project"].clone())?;
    assert!(
        proj_tier.contains(&"billing-migrations".to_string()),
        "billing-migrations must be in project tier: {tb}"
    );
    // axum-handler should be in the curated bucket, not project.
    let curated_tier: Vec<String> = serde_json::from_value(tb["curated"].clone())?;
    assert!(
        curated_tier.contains(&"axum-handler".to_string()),
        "axum-handler must be in curated tier: {tb}"
    );

    Ok(())
}

/// `?project=does-not-exist&stack=rust` — unknown project slug is a soft miss:
/// no error, just falls back to stack-based tiers normally.
#[tokio::test]
async fn bootstrap_project_not_found_falls_back_cleanly() -> Result<()> {
    let h = boot(Arc::new(StubEmbedder)).await?;
    let c = client();

    // No project created. Publish a skill so tier 2 has something.
    publish_skill(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "axum-handler",
        "Pattern for axum handlers",
        &["rust"],
    )
    .await?;

    let body = bootstrap_get(
        &c,
        &h,
        "acme",
        &h.acme_token,
        "project=does-not-exist&stack=rust&debug=1",
    )
    .await?;

    // Must not be an error response.
    assert!(
        body.get("error").is_none(),
        "non-existent project must not produce error: {body}"
    );

    // `project` field absent or null in response.
    let proj = &body["project"];
    assert!(
        proj.is_null() || !proj.is_string(),
        "project field must be absent/null for unknown project: {body}"
    );

    // Skills still populated via stack tiers.
    let skills: Vec<String> = serde_json::from_value(body["skills"].clone())?;
    assert!(
        skills.contains(&"axum-handler".to_string()),
        "stack-tier skills must still appear when project is unknown: {skills:?}"
    );

    Ok(())
}
