//! Integration test for the stack-mappings admin REST surface.
//!
//! Covers:
//!   1. POST adds a row; GET returns it.
//!   2. POST on the same (stack, skill) is idempotent.
//!   3. DELETE removes; subsequent GET drops the row.
//!   4. DELETE on a non-existent mapping → 404.
//!   5. Cross-tenant isolation.
//!   6. Non-admin caller → 403.
//!   7. Empty stack / skill → 400.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

struct Harness {
    base: String,
    acme_admin: String,
    acme_reader: String,
    globex_admin: String,
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
    let pool = PgPoolOptions::new().max_connections(4).connect(&db_url).await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex", "team").await?;
    let acme_admin = admin::create_token(&pool, "acme", "admin", "tenant:admin skills:read").await?.raw_token;
    let acme_reader = admin::create_token(&pool, "acme", "reader", "skills:read").await?.raw_token;
    let globex_admin = admin::create_token(&pool, "globex", "admin", "tenant:admin skills:read").await?.raw_token;

    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
        db_pool_size: 20,
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
    };
    let state = state::AppState::new(&cfg).await?;
    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    Ok(Harness {
        base: format!("http://{addr}"),
        acme_admin,
        acme_reader,
        globex_admin,
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

fn c() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap()
}
fn req(cl: &reqwest::Client, m: reqwest::Method, base: &str, p: &str, tenant: &str) -> reqwest::RequestBuilder {
    cl.request(m, format!("{base}{p}")).header("x-skill-pool-tenant", tenant)
}
fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

#[tokio::test]
async fn stack_mappings_api_round_trip() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // 1. POST one mapping.
    let r = authed(
        req(&cl, reqwest::Method::POST, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    )
    .json(&json!({"stack": "rust", "skill": "axum-handler"}))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);

    // 2. GET surfaces it.
    let list: Vec<Value> = authed(
        req(&cl, reqwest::Method::GET, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["stack"], "rust");
    assert_eq!(list[0]["skill"], "axum-handler");

    // 3. POST same pair → idempotent (still one row).
    let r = authed(
        req(&cl, reqwest::Method::POST, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    )
    .json(&json!({"stack": "rust", "skill": "axum-handler"}))
    .send().await?;
    assert_eq!(r.status().as_u16(), 200);
    let list: Vec<Value> = authed(
        req(&cl, reqwest::Method::GET, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(list.len(), 1, "{list:?}");

    // 4. Add a second.
    let _ = authed(
        req(&cl, reqwest::Method::POST, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    )
    .json(&json!({"stack": "rust", "skill": "sqlx-migrations"}))
    .send().await?;
    let list: Vec<Value> = authed(
        req(&cl, reqwest::Method::GET, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(list.len(), 2);

    // 5. DELETE one.
    let r = authed(
        req(&cl, reqwest::Method::DELETE, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    )
    .json(&json!({"stack": "rust", "skill": "sqlx-migrations"}))
    .send().await?;
    assert_eq!(r.status().as_u16(), 204);
    let list: Vec<Value> = authed(
        req(&cl, reqwest::Method::GET, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(list.len(), 1);
    assert_eq!(list[0]["skill"], "axum-handler");

    // 6. DELETE non-existent → 404.
    let r = authed(
        req(&cl, reqwest::Method::DELETE, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    )
    .json(&json!({"stack": "ghost", "skill": "void"}))
    .send().await?;
    assert_eq!(r.status().as_u16(), 404);

    // 7. Cross-tenant isolation — globex can't see acme's row.
    let list: Vec<Value> = authed(
        req(&cl, reqwest::Method::GET, &h.base, "/v1/tenant/stack-mappings", "globex"),
        &h.globex_admin,
    ).send().await?.json().await?;
    assert_eq!(list.len(), 0, "{list:?}");

    // 8. Non-admin caller (acme reader) → 403.
    let r = authed(
        req(&cl, reqwest::Method::GET, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_reader,
    ).send().await?;
    assert_eq!(r.status().as_u16(), 403);

    // 9. Empty stack / skill → 400.
    let r = authed(
        req(&cl, reqwest::Method::POST, &h.base, "/v1/tenant/stack-mappings", "acme"),
        &h.acme_admin,
    )
    .json(&json!({"stack": "  ", "skill": "x"}))
    .send().await?;
    assert_eq!(r.status().as_u16(), 400);

    Ok(())
}
