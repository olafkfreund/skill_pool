//! Integration test for issue #30 — POST /v1/plugins happy path +
//! cross-tenant isolation.
//!
//! Flow:
//!   1. Boot pgvector + the full router (no Redis — rate limiter fails
//!      open with no Redis, mirroring `rate_limits::rate_limit_fails_open_without_redis`).
//!   2. Create tenants `acme` + `globex`. Mint a `skills:read skills:publish`
//!      token for each.
//!   3. Seed three published skills into `acme` only (one skill / one agent /
//!      one command — covers every valid `content_kind`).
//!   4. POST /v1/plugins as acme with manifest + the three contents → 201.
//!   5. GET /v1/plugins as acme → list includes the plugin.
//!   6. GET /v1/plugins as globex → list is empty (cross-tenant isolation).
//!   7. GET /v1/plugins/{slug} as acme → 200; as globex → 404.

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

/// Three published catalog items in the given tenant — one per kind so
/// the test exercises every valid `content_kind` value in a single
/// publish.
async fn seed_three_published_skills(pool: &PgPool, tid: Uuid) -> Result<()> {
    for (slug, kind, version) in [
        ("fmt", "skill", "1.0.0"),
        ("lint", "agent", "0.2.0"),
        ("scaffold", "command", "0.1.0"),
    ] {
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
    }
    Ok(())
}

#[tokio::test]
async fn publish_lists_and_isolates_per_tenant() -> Result<()> {
    let (pool, h) = boot().await?;
    let acme = tenant_id(&pool, "acme").await?;
    seed_three_published_skills(&pool, acme).await?;

    let client = reqwest::Client::new();

    // ----- 4. POST /v1/plugins as acme -------------------------------
    let body = json!({
        "slug": "rust-toolkit",
        "manifest": {
            "name": "Rust Toolkit",
            "version": "1.0.0",
            "description": "Rust dev essentials",
            "tags": ["rust", "tooling"],
        },
        "contents": [
            { "kind": "skill",   "slug": "fmt",      "version": "1.0.0" },
            { "kind": "agent",   "slug": "lint",     "version": "0.2.0" },
            { "kind": "command", "slug": "scaffold", "version": "0.1.0" },
        ],
        "sourcing_mode": "internal",
    });
    let resp = client
        .post(format!("{}/v1/plugins", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let payload: Value = resp.json().await?;
    assert_eq!(status.as_u16(), 201, "publish failed: {payload}");
    assert_eq!(payload["slug"], "rust-toolkit");
    assert_eq!(payload["version"], "1.0.0");
    assert_eq!(payload["status"], "published");
    assert_eq!(payload["sourcing_mode"], "internal");
    assert_eq!(payload["contents"].as_array().unwrap().len(), 3);

    // ----- 5. List as acme — sees the plugin --------------------------
    let resp = client
        .get(format!("{}/v1/plugins", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let listing: Value = resp.json().await?;
    let items = listing["items"].as_array().expect("items array");
    assert_eq!(items.len(), 1, "acme list should have the plugin: {listing}");
    assert_eq!(items[0]["slug"], "rust-toolkit");
    assert_eq!(items[0]["tags"], json!(["rust", "tooling"]));

    // ----- 6. List as globex — must be empty (isolation) --------------
    let resp = client
        .get(format!("{}/v1/plugins", h.base))
        .header("x-skill-pool-tenant", "globex")
        .header("authorization", format!("Bearer {}", h.globex_token))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let listing: Value = resp.json().await?;
    assert!(
        listing["items"].as_array().unwrap().is_empty(),
        "globex must not see acme's plugin: {listing}"
    );

    // ----- 7. Detail — acme 200, globex 404 ---------------------------
    let resp = client
        .get(format!("{}/v1/plugins/rust-toolkit", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let detail: Value = resp.json().await?;
    assert_eq!(detail["slug"], "rust-toolkit");
    assert_eq!(detail["contents"].as_array().unwrap().len(), 3);

    let resp = client
        .get(format!("{}/v1/plugins/rust-toolkit", h.base))
        .header("x-skill-pool-tenant", "globex")
        .header("authorization", format!("Bearer {}", h.globex_token))
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        404,
        "globex must not be able to read acme's plugin by slug"
    );

    drop(pool);
    Ok(())
}

#[tokio::test]
async fn archive_flips_status_and_hides_from_listing() -> Result<()> {
    let (pool, h) = boot().await?;
    let acme = tenant_id(&pool, "acme").await?;
    seed_three_published_skills(&pool, acme).await?;
    let client = reqwest::Client::new();

    // Publish two versions of the same plugin so the archive only
    // hides one of them, and version-history still shows both.
    for version in ["1.0.0", "1.1.0"] {
        let body = json!({
            "slug": "kit",
            "manifest": {
                "name": "Kit",
                "version": version,
                "description": "test plugin",
            },
            "contents": [{ "kind": "skill", "slug": "fmt", "version": "1.0.0" }],
            "sourcing_mode": "internal",
        });
        let resp = client
            .post(format!("{}/v1/plugins", h.base))
            .header("x-skill-pool-tenant", "acme")
            .header("authorization", format!("Bearer {}", h.acme_token))
            .json(&body)
            .send()
            .await?;
        assert_eq!(resp.status().as_u16(), 201, "publish {version} failed");
    }

    // Archive 1.0.0.
    let resp = client
        .delete(format!("{}/v1/plugins/kit/versions/1.0.0", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 204);

    // Second archive is a no-op → 404 (already archived).
    let resp = client
        .delete(format!("{}/v1/plugins/kit/versions/1.0.0", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 404);

    // Detail still shows 1.1.0 (DISTINCT ON slug → latest published).
    let resp = client
        .get(format!("{}/v1/plugins/kit", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .send()
        .await?;
    let detail: Value = resp.json().await?;
    assert_eq!(detail["version"], "1.1.0");

    // Version history surfaces BOTH (one archived, one published).
    let resp = client
        .get(format!("{}/v1/plugins/kit/versions", h.base))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {}", h.acme_token))
        .send()
        .await?;
    let versions: Value = resp.json().await?;
    let arr = versions.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    let statuses: Vec<&str> = arr.iter().map(|v| v["status"].as_str().unwrap()).collect();
    assert!(statuses.contains(&"archived"));
    assert!(statuses.contains(&"published"));

    drop(pool);
    Ok(())
}
