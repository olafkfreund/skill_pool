//! Integration tests for the validation layer of POST /v1/plugins (#30).
//!
//! Each `#[tokio::test]` exercises one rejection rule end-to-end so a
//! failure points directly at the broken branch:
//!
//!   - missing required manifest fields  → 422 with field-level errors
//!   - manifest > 256 KiB                → 413
//!   - content slugs that don't exist     → 422 (per-index field error)
//!   - content slugs from another tenant  → 422 (tenant-scoped lookup
//!     never sees them, so they read as "not published in this tenant")
//!   - duplicate `(slug, version)` on second publish → 409

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

use skill_pool_server::{admin, config, routes, state};

struct Harness {
    base: String,
    acme_token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
}

async fn boot() -> Result<(PgPool, Harness)> {
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

    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
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
    unsafe { std::env::remove_var("SKILL_POOL_REDIS_URL") };
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok((
        pool,
        Harness {
            base: format!("http://{addr}"),
            acme_token,
            _pg: pg,
        },
    ))
}

async fn tenant_id(pool: &PgPool, slug: &str) -> Result<Uuid> {
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM tenants WHERE slug = $1")
        .bind(slug)
        .fetch_one(pool)
        .await?;
    Ok(id)
}

async fn insert_published_skill(
    pool: &PgPool,
    tid: Uuid,
    slug: &str,
    kind: &str,
    version: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO skills \
           (tenant_id, slug, version, description, when_to_use, tags, \
            bundle_uri, bundle_sha256, kind, status) \
         VALUES ($1, $2, $3, 'd', NULL, '{}', '/k', 'h', $4, 'published')",
    )
    .bind(tid)
    .bind(slug)
    .bind(version)
    .bind(kind)
    .execute(pool)
    .await?;
    Ok(())
}

async fn post_publish(client: &reqwest::Client, h: &Harness, body: &Value) -> reqwest::Response {
    client
        .post(format!("{}/v1/plugins", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .json(body)
        .send()
        .await
        .expect("publish request")
}

#[tokio::test]
async fn rejects_manifest_missing_required_fields() -> Result<()> {
    let (pool, h) = boot().await?;
    let client = reqwest::Client::new();

    // Missing `name`, `version`, AND `description` — we expect all three
    // fields enumerated in one 422 (the validator collects errors so the
    // client renders every field at once).
    let body = json!({
        "slug": "kit",
        "manifest": { "tags": [] },
        "contents": [],
        "sourcing_mode": "internal",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 422);
    let payload: Value = resp.json().await?;
    assert_eq!(payload["error"], "unprocessable_entity");
    let fields = payload["fields"].as_object().expect("fields object");
    assert!(fields.contains_key("name"),        "expected `name`: {payload}");
    assert!(fields.contains_key("version"),     "expected `version`: {payload}");
    assert!(fields.contains_key("description"), "expected `description`: {payload}");

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn rejects_oversize_manifest_413() -> Result<()> {
    let (pool, h) = boot().await?;
    let client = reqwest::Client::new();

    // Pad an opaque field past 256 KiB. We use a single ~300 KiB string
    // so the canonical re-serialisation is just over the cap regardless
    // of whitespace.
    let pad = "a".repeat(300 * 1024);
    let body = json!({
        "slug": "big",
        "manifest": {
            "name": "Big",
            "version": "1.0.0",
            "description": "way too large",
            "pad": pad,
        },
        "contents": [],
        "sourcing_mode": "internal",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 413);

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn rejects_unknown_content_slugs() -> Result<()> {
    let (pool, h) = boot().await?;
    let client = reqwest::Client::new();

    let body = json!({
        "slug": "kit",
        "manifest": {
            "name": "Kit",
            "version": "1.0.0",
            "description": "test",
        },
        "contents": [
            { "kind": "skill", "slug": "does-not-exist", "version": "9.9.9" },
        ],
        "sourcing_mode": "internal",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 422);
    let payload: Value = resp.json().await?;
    let fields = payload["fields"].as_object().unwrap();
    assert!(
        fields.contains_key("contents[0]"),
        "expected `contents[0]` error: {payload}"
    );

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn rejects_cross_tenant_content_slugs() -> Result<()> {
    let (pool, h) = boot().await?;
    let globex = tenant_id(&pool, "globex").await?;
    // Seed a skill in *globex*. Acme must not be able to bundle it.
    insert_published_skill(&pool, globex, "secret", "skill", "1.0.0").await?;

    let client = reqwest::Client::new();
    let body = json!({
        "slug": "leaky",
        "manifest": {
            "name": "Leaky",
            "version": "1.0.0",
            "description": "tries to bundle another tenant's skill",
        },
        "contents": [
            { "kind": "skill", "slug": "secret", "version": "1.0.0" },
        ],
        "sourcing_mode": "internal",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(
        resp.status().as_u16(),
        422,
        "cross-tenant slug must read as not-published in this tenant"
    );
    let payload: Value = resp.json().await?;
    let msg = payload["fields"]["contents[0]"].as_str().unwrap_or("");
    assert!(
        msg.contains("not published in this tenant"),
        "expected tenant-scoped wording: {payload}"
    );

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn duplicate_slug_version_returns_409() -> Result<()> {
    let (pool, h) = boot().await?;
    let acme = tenant_id(&pool, "acme").await?;
    insert_published_skill(&pool, acme, "fmt", "skill", "1.0.0").await?;

    let client = reqwest::Client::new();
    let body = json!({
        "slug": "kit",
        "manifest": {
            "name": "Kit",
            "version": "1.0.0",
            "description": "test",
        },
        "contents": [{ "kind": "skill", "slug": "fmt", "version": "1.0.0" }],
        "sourcing_mode": "internal",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 201, "first publish must succeed");

    // Same body again → 409 Conflict.
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 409);

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn rejects_invalid_content_kind() -> Result<()> {
    // Smoke-cover the per-index kind validator (mentioned in the plan
    // table). 422 with the per-index field key.
    let (pool, h) = boot().await?;
    let client = reqwest::Client::new();

    let body = json!({
        "slug": "kit",
        "manifest": {
            "name": "Kit",
            "version": "1.0.0",
            "description": "test",
        },
        "contents": [{ "kind": "plugin", "slug": "x", "version": "1.0.0" }],
        "sourcing_mode": "internal",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 422);
    let payload: Value = resp.json().await?;
    let fields = payload["fields"].as_object().unwrap();
    assert!(
        fields.contains_key("contents[0].kind"),
        "expected `contents[0].kind` error: {payload}"
    );

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn external_mode_requires_git_url() -> Result<()> {
    let (pool, h) = boot().await?;
    let client = reqwest::Client::new();

    let body = json!({
        "slug": "ext",
        "manifest": {
            "name": "Ext",
            "version": "1.0.0",
            "description": "test",
        },
        "contents": [],
        "sourcing_mode": "external",
    });
    let resp = post_publish(&client, &h, &body).await;
    assert_eq!(resp.status().as_u16(), 422);
    let payload: Value = resp.json().await?;
    assert!(
        payload["fields"]["external_git_url"].is_string(),
        "expected field error: {payload}"
    );

    drop(pool);
    Ok(())
}
