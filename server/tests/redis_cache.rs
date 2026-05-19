//! Integration tests for the Redis read-through cache (#9 §L36, #10 §A).
//!
//! Three guarantees:
//!
//!   1. **Cache hit serves stale.** A GET /v1/theme populates Redis;
//!      a follow-up GET still returns the cached value after the
//!      underlying DB row is deleted out-of-band. This proves the
//!      cache is actually being read.
//!
//!   2. **Invalidation works.** A PUT /v1/theme invalidates the entry;
//!      the next GET sees the new brand_name even within the TTL.
//!
//!   3. **Graceful fallback.** Pointing `redis_url` at a bogus host
//!      keeps the server working: every request still succeeds, just
//!      without a cache (the `AppState::new` connect failure logs at
//!      WARN and `state.redis()` returns `None`).

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis as RedisContainer;

use skill_pool_server::{admin, config, routes, state};

struct Harness {
    base: String,
    admin_token: String,
    db: sqlx::PgPool,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _redis: Option<testcontainers::ContainerAsync<RedisContainer>>,
    _storage_dir: tempfile::TempDir,
}

/// Boot a server with optional Redis. `redis_url_override = Some(url)`
/// pins the config to that URL — pass a bogus host to exercise the
/// graceful-fallback path.
async fn boot(redis_url_override: Option<String>) -> Result<Harness> {
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
    let admin_token = admin::create_token(
        &pool,
        "acme",
        "admin",
        "tenant:admin skills:read skills:publish",
    )
    .await?
    .raw_token;

    // Boot Redis (real one) unless the caller wants the broken-URL path.
    let (redis_url, redis_container) = match redis_url_override {
        Some(url) => (Some(url), None),
        None => {
            let r = RedisContainer::default().start().await?;
            let port = r.get_host_port_ipv4(6379).await?;
            (Some(format!("redis://127.0.0.1:{port}")), Some(r))
        }
    };

    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
        redis_url,
        db_pool_size: 20,
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
        queue_enabled: None,
    };
    let app_state = state::AppState::new(&cfg).await?;
    let app = routes::router(app_state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        admin_token,
        db: pool,
        _pg: pg,
        _redis: redis_container,
        _storage_dir: storage_dir,
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

/// 1. GET caches; second GET returns the cached payload even after the
///    underlying DB row is mutated out-of-band.
#[tokio::test]
async fn cache_serves_after_db_row_changes() -> Result<()> {
    let h = boot(None).await?;
    let c = client();

    // First GET — DB has no tenant_theme row yet, so the response is
    // the default (brand_name == slug).
    let first: Value = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(first["brand_name"], "acme");

    // Insert a row out-of-band. If the cache weren't being read, the
    // next GET would pick this up. With the cache, the old "default"
    // payload should still be served.
    sqlx::query(
        "INSERT INTO tenant_theme \
         (tenant_id, brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, border, radius) \
         SELECT id, 'OUT_OF_BAND', '#000000', '#ffffff', '#000000', '#ffffff', '#000000', '#f1f5f9', '#475569', '#e2e8f0', '0.5rem' \
         FROM tenants WHERE slug = 'acme' \
         ON CONFLICT (tenant_id) DO UPDATE SET brand_name = EXCLUDED.brand_name",
    )
    .execute(&h.db)
    .await?;

    let second: Value = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(
        second["brand_name"], "acme",
        "second GET should serve cached value; got {second}"
    );

    Ok(())
}

/// 2. PUT invalidates the cache; subsequent GET sees the new value.
#[tokio::test]
async fn put_invalidates_cache() -> Result<()> {
    let h = boot(None).await?;
    let c = client();

    // Warm the cache.
    let _ = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .bytes()
        .await?;

    // PUT a new theme.
    let new_theme = serde_json::json!({
        "brand_name": "Updated",
        "primary": "#2563eb",
        "primary_fg": "#ffffff",
        "accent": "#0ea5e9",
        "bg": "#ffffff",
        "fg": "#0f172a",
        "muted": "#f1f5f9",
        "muted_fg": "#475569",
        "border": "#e2e8f0",
        "radius": "0.5rem",
        "footer_branding": true
    });
    let resp = c
        .put(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.admin_token)
        .json(&new_theme)
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200, "PUT failed");

    // Next GET should reflect the new brand_name — proving the
    // invalidate hook fired before the next read.
    let after: Value = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(after["brand_name"], "Updated", "after PUT: {after}");

    Ok(())
}

/// 3. When `redis_url` points at a dead host, the server still serves
///    requests (graceful fallback). `state.redis()` returns None at
///    boot time after the connect fails, so every cache call becomes
///    a direct DB hit.
#[tokio::test]
async fn graceful_fallback_when_redis_unreachable() -> Result<()> {
    // Use an unreachable port. 127.0.0.1:1 is reliably closed and the
    // ConnectionManager initial connect will fail fast.
    let h = boot(Some("redis://127.0.0.1:1".into())).await?;
    let c = client();

    let resp = c
        .get(format!("{}/v1/theme", h.base))
        .header("x-skill-pool-tenant", "acme")
        .send()
        .await?;
    assert_eq!(resp.status().as_u16(), 200);
    let body: Value = resp.json().await?;
    assert_eq!(body["brand_name"], "acme");

    Ok(())
}
