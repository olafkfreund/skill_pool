//! Issue #31 — confirm `.claude-plugin/marketplace.json` + the git
//! endpoint are unauthenticated (no `Authorization` header needed) but
//! the per-tenant rate limiter still applies.
//!
//! Two assertions:
//!   1. Without any `Authorization` header, GET marketplace.json and
//!      GET /git/plugins/<slug>.git/info/refs both return 200.
//!   2. With a tight per-tenant rate limit and a burst of requests,
//!      we observe 429s — matching `tests/rate_limit_plugins.rs`.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis;

use skill_pool_server::{admin, cache, config, routes, state};

#[tokio::test]
async fn marketplace_and_git_endpoints_are_public_and_rate_limited() -> Result<()> {
    // Need Redis for rate limiting; the marketplace + git routes are
    // not in `SKIP_PATHS`, so the limiter applies once we have a tenant.
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

    let rd = Redis::default().start().await?;
    let rd_port = rd.get_host_port_ipv4(6379).await?;
    let redis_url = format!("redis://127.0.0.1:{rd_port}");
    let redis = cache::connect(&redis_url).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());
    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    // Tight cap so 8 requests cross the threshold quickly.
    admin::set_tenant_rate_limits(&pool, "acme", Some(3), Some(3), false).await?;
    let token = admin::create_token(&pool, "acme", "test", "skills:read skills:publish")
        .await?
        .raw_token;

    // Seed a skill + plugin so info/refs has something real to serve.
    let acme: uuid::Uuid =
        sqlx::query_scalar("SELECT id FROM tenants WHERE slug = 'acme'")
            .fetch_one(&pool)
            .await?;
    sqlx::query(
        "INSERT INTO skills \
           (tenant_id, slug, version, description, when_to_use, tags, \
            bundle_uri, bundle_sha256, kind, status) \
         VALUES ($1, 'fmt', '1.0.0', 'd', NULL, '{}', '/k', 'h', 'skill', 'published')",
    )
    .bind(acme)
    .execute(&pool)
    .await?;

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
    let app_state = state::AppState::new_with_redis(&cfg, redis.clone()).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");
    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()?;

    // Publish an internal plugin so the bare repo exists + info/refs can
    // resolve. Done via the authenticated API; public-read assertions
    // below cover the GETs.
    let resp = c
        .post(format!("{base}/v1/plugins"))
        .header("x-skill-pool-tenant", "acme")
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({
            "slug": "kit",
            "manifest": {
                "name": "kit", "version": "1.0.0", "description": "test",
            },
            "contents": [
                { "kind": "skill", "slug": "fmt", "version": "1.0.0" }
            ],
            // Use `mirror` to skip the git materialiser (the skill row
            // has a fake bundle_uri); marketplace entry still gets
            // written and points at the local git endpoint. info/refs
            // resolves the plugin row but the repo path won't exist —
            // we'll seed an empty bare repo manually so the smart
            // protocol exchange completes.
            "sourcing_mode": "mirror",
            "upstream_url": "https://example.com/kit.git",
        }))
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 201);

    // Manually init the bare repo for the mirror plugin so info/refs has
    // something on disk. Real mirror mode would do this via the import
    // worker (#36 follow-up).
    let repo_path = storage_dir
        .path()
        .join(acme.to_string())
        .join("plugins")
        .join("kit.git");
    std::fs::create_dir_all(repo_path.parent().unwrap())?;
    git2::Repository::init_bare(&repo_path)?;

    // ----- 1. No auth header → 200 ------------------------------------
    let resp = c
        .get(format!("{base}/.claude-plugin/marketplace.json"))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "marketplace.json must be public-read"
    );

    let resp = c
        .get(format!(
            "{base}/git/plugins/kit.git/info/refs?service=git-upload-pack"
        ))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(
        resp.status().as_u16(),
        200,
        "info/refs must be public-read"
    );

    // ----- 2. Rate limit still applies --------------------------------
    let mut allowed = 0;
    let mut denied = 0;
    for _ in 0..8 {
        let resp = c
            .get(format!("{base}/.claude-plugin/marketplace.json"))
            .header("x-skill-pool-tenant", "acme")
            .send()
            .await?;
        match resp.status().as_u16() {
            200 | 304 => allowed += 1,
            429 => denied += 1,
            s => panic!("unexpected status {s} on marketplace.json"),
        }
    }
    // We already burnt 1 + 1 + 1 above (publish + marketplace.json + info/refs),
    // so the 3-burst budget allows ~0 here and we should see 8 denials —
    // or up to a couple of allowances depending on the second window
    // boundary. Assert at least one denial to prove the limiter wired.
    assert!(
        denied >= 1,
        "expected at least one rate-limited response (allowed={allowed} denied={denied})"
    );

    drop(pool);
    Ok(())
}
