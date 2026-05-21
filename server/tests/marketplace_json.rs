//! Issue #31 — `.claude-plugin/marketplace.json` end-to-end.
//!
//! Flow:
//!   1. Boot pgvector + the full router (no Redis — rate limiter fails
//!      open, matching `tests/plugin_publish.rs`).
//!   2. Create tenants `acme` + `globex`. Mint write tokens for each.
//!   3. Seed one published skill in each tenant.
//!   4. Publish one plugin per tenant via POST /v1/plugins.
//!   5. GET /.claude-plugin/marketplace.json as acme (no auth header) →
//!      exactly one plugin in the list with the right slug and a source
//!      URL pointing at `<host>/git/plugins/<slug>.git`.
//!   6. GET /.claude-plugin/marketplace.json as globex → exactly one
//!      plugin, and it's the globex one (cross-tenant isolation).
//!   7. ETag round-trip: send the prior ETag in `If-None-Match` → 304.
//!   8. Empty marketplace: a third tenant `wayne` with no plugins → 200
//!      with `plugins: []`.

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
    globex_token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
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

    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex", "team").await?;
    admin::create_tenant(&pool, "wayne", "Wayne Enterprises", "team").await?;
    let acme_token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;
    let globex_token = admin::create_token(&pool, "globex", "test", "skills:read skills:publish")
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
    // SAFETY: leaf integration test, no other thread touches env.
    unsafe { std::env::remove_var("SKILL_POOL_REDIS_URL") };
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let h = Harness {
        base: format!("http://{addr}"),
        acme_token,
        globex_token,
        _pg: pg,
        _storage_dir: storage_dir,
    };
    Ok((pool, h))
}

async fn tenant_id(pool: &PgPool, slug: &str) -> Result<Uuid> {
    let (id,): (Uuid,) = sqlx::query_as("SELECT id FROM tenants WHERE slug = $1")
        .bind(slug)
        .fetch_one(pool)
        .await?;
    Ok(id)
}

/// Seed a single published skill row for content-validation in the publish.
/// We use the SQL-direct seeding shortcut from `tests/plugin_publish.rs` —
/// the bundle_uri is fake because this test never materialises a git tree
/// (that's the next test's job). See `plugin_git_clone.rs` for the real
/// bundle path.
async fn seed_skill(pool: &PgPool, tid: Uuid, slug: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO skills \
           (tenant_id, slug, version, description, when_to_use, tags, \
            bundle_uri, bundle_sha256, kind, status) \
         VALUES ($1, $2, '1.0.0', 'd', NULL, '{}', '/fake', 'h', 'skill', 'published')",
    )
    .bind(tid)
    .bind(slug)
    .execute(pool)
    .await?;
    Ok(())
}

async fn publish_plugin(
    client: &reqwest::Client,
    base: &str,
    tenant: &str,
    token: &str,
    slug: &str,
    skill_slug: &str,
) -> Result<()> {
    let body = json!({
        "slug": slug,
        "manifest": {
            "name": slug,
            "version": "1.0.0",
            "description": format!("{slug} test plugin"),
            "keywords": ["test"],
        },
        "contents": [
            { "kind": "skill", "slug": skill_slug, "version": "1.0.0" },
        ],
        // Internal would trigger git-tree materialisation; the empty
        // bundle_uri in seed_skill would fail that. External skips both
        // the materialiser AND uses the upstream URL verbatim — but for
        // a marketplace.json shape test we want skill-pool's own git
        // URL. The hook treats `mirror` the same as `internal` for
        // source URL purposes WITHOUT trying to materialise — exactly
        // what this test needs.
        "sourcing_mode": "mirror",
        "upstream_url": format!("https://example.com/{slug}.git"),
    });
    let resp = client
        .post(format!("{base}/v1/plugins"))
        .header("x-skill-pool-tenant", tenant)
        .header("authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let payload: Value = resp.json().await?;
    if status.as_u16() != 201 {
        anyhow::bail!("publish {slug}@{tenant} failed: {status} {payload}");
    }
    Ok(())
}

#[tokio::test]
async fn marketplace_json_is_per_tenant_with_local_git_source() -> Result<()> {
    let (pool, h) = boot().await?;
    let acme = tenant_id(&pool, "acme").await?;
    let globex = tenant_id(&pool, "globex").await?;
    seed_skill(&pool, acme, "fmt").await?;
    seed_skill(&pool, globex, "lint").await?;

    let client = reqwest::Client::new();
    publish_plugin(&client, &h.base, "acme", &h.acme_token, "acme-toolkit", "fmt").await?;
    publish_plugin(&client, &h.base, "globex", &h.globex_token, "globex-pack", "lint").await?;

    // ----- acme's marketplace -----------------------------------------
    let resp = client
        .get(format!("{}/.claude-plugin/marketplace.json", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let etag = resp
        .headers()
        .get("etag")
        .map(|v| v.to_str().unwrap().to_string())
        .expect("etag header");
    assert_eq!(
        resp.headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("public, max-age=60"),
    );
    let body: Value = resp.json().await?;
    assert_eq!(body["name"], "acme");
    assert_eq!(body["owner"]["name"], "Acme Corp");
    let plugins = body["plugins"].as_array().expect("plugins array");
    assert_eq!(plugins.len(), 1, "acme should see exactly one plugin: {body}");
    assert_eq!(plugins[0]["name"], "acme-toolkit");
    assert_eq!(plugins[0]["version"], "1.0.0");
    assert_eq!(plugins[0]["description"], "acme-toolkit test plugin");
    assert_eq!(plugins[0]["keywords"], json!(["test"]));
    let source = &plugins[0]["source"];
    assert_eq!(source["source"], "url");
    let url = source["url"].as_str().unwrap();
    assert!(
        url.starts_with("http://") && url.ends_with("/git/plugins/acme-toolkit.git"),
        "expected local git URL, got `{url}`"
    );

    // ----- globex's marketplace — only globex's plugin ----------------
    let resp = client
        .get(format!("{}/.claude-plugin/marketplace.json", h.base))
        .header("x-skill-pool-tenant", "globex")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["name"], "globex");
    let plugins = body["plugins"].as_array().expect("plugins array");
    assert_eq!(plugins.len(), 1);
    assert_eq!(plugins[0]["name"], "globex-pack");
    // Cross-tenant assertion: globex must NOT see acme's plugin.
    assert!(
        plugins.iter().all(|p| p["name"] != "acme-toolkit"),
        "cross-tenant leak: globex sees acme's plugin"
    );

    // ----- ETag conditional GET ---------------------------------------
    let resp = client
        .get(format!("{}/.claude-plugin/marketplace.json", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("if-none-match", &etag)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 304, "stale-ETag should yield 304");

    // ----- Empty marketplace (wayne has zero published plugins) -------
    let resp = client
        .get(format!("{}/.claude-plugin/marketplace.json", h.base))
        .header("x-skill-pool-tenant", "wayne")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["name"], "wayne");
    assert_eq!(body["plugins"], json!([]), "empty tenant should serve empty plugins[]");

    drop(pool);
    Ok(())
}
