//! Integration tests for the Projects REST API.
//!
//! Covers:
//!   1. Full CRUD round-trip: create → detail → update → delete.
//!   2. Items round-trip: PUT items → GET detail shows them in position order.
//!   3. PUT items is atomic (replaces entirely; re-PUT shorter list).
//!   4. Cross-tenant isolation: tenant B cannot see tenant A's projects.
//!   5. Non-admin token → 403 for admin routes; resolve is any-member.
//!   6. Missing project → 404.
//!   7. Duplicate slug → 409.
//!   8. resolve?remote= normalises SSH↔HTTPS URLs.
//!   9. Invalid item kind → 400.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{admin, config, routes, state};

struct Harness {
    base: String,
    acme_admin: String,
    acme_reader: String,
    globex_admin: String,
    pool: sqlx::PgPool,
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

    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    admin::create_tenant(&pool, "globex", "Globex Corp", "team").await?;

    let acme_admin = admin::create_token(&pool, "acme", "admin", "tenant:admin skills:read")
        .await?
        .raw_token;
    let acme_reader = admin::create_token(&pool, "acme", "reader", "skills:read")
        .await?
        .raw_token;
    let globex_admin = admin::create_token(&pool, "globex", "admin", "tenant:admin skills:read")
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
        acme_admin,
        acme_reader,
        globex_admin,
        pool,
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

fn c() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn req(
    cl: &reqwest::Client,
    m: reqwest::Method,
    base: &str,
    p: &str,
    tenant: &str,
) -> reqwest::RequestBuilder {
    cl.request(m, format!("{base}{p}"))
        .header("x-skill-pool-tenant", tenant)
}

fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

/// Full CRUD + items round-trip for a single project.
#[tokio::test]
async fn projects_crud_and_items_round_trip() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // 1. Create a project.
    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "slug": "acme-billing",
        "name": "Acme Billing Service",
        "description": "Skills for the billing micro-service",
        "git_remote": "git@github.com:acme/billing.git"
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 201, "{}", r.text().await?);
    let created: Value = r.json().await?;
    assert_eq!(created["slug"], "acme-billing");
    assert_eq!(created["name"], "Acme Billing Service");
    // git_remote should be normalized (SSH → HTTPS, .git stripped)
    assert_eq!(
        created["git_remote"],
        "https://github.com/acme/billing",
        "git_remote not normalized: {created}"
    );

    // 2. List — one project.
    let list: Vec<Value> = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(list.len(), 1, "expected 1 project: {list:?}");
    assert_eq!(list[0]["slug"], "acme-billing");

    // 3. GET detail — no items yet.
    let detail: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/acme-billing",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(detail["slug"], "acme-billing");
    let items = detail["items"].as_array().unwrap();
    assert!(items.is_empty(), "expected no items yet: {detail}");

    // 4. PUT items — three items in specific order.
    let r = authed(
        req(
            &cl,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/projects/acme-billing/items",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!([
        {"slug": "sqlx-migrations", "kind": "skill"},
        {"slug": "axum-handler", "kind": "skill"},
        {"slug": "billing-agent", "kind": "agent"}
    ]))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 204, "{}", r.text().await?);

    // 5. GET detail — items present in position order.
    let detail: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/acme-billing",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    let items = detail["items"].as_array().unwrap();
    assert_eq!(items.len(), 3, "expected 3 items: {detail}");
    assert_eq!(items[0]["skill_slug"], "sqlx-migrations");
    assert_eq!(items[0]["position"], 0);
    assert_eq!(items[1]["skill_slug"], "axum-handler");
    assert_eq!(items[1]["position"], 1);
    assert_eq!(items[2]["skill_slug"], "billing-agent");
    assert_eq!(items[2]["kind"], "agent");
    assert_eq!(items[2]["position"], 2);

    // 6. Re-PUT with shorter list → atomic replacement (only 1 item).
    let r = authed(
        req(
            &cl,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/projects/acme-billing/items",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!([
        {"slug": "axum-handler", "kind": "skill"}
    ]))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 204);

    let detail: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/acme-billing",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    let items = detail["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "expected 1 item after re-PUT: {items:?}");
    assert_eq!(items[0]["skill_slug"], "axum-handler");

    // 7. PATCH — update name and description.
    let r = authed(
        req(
            &cl,
            reqwest::Method::PATCH,
            &h.base,
            "/v1/tenant/projects/acme-billing",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "name": "Acme Billing v2",
        "description": "Updated description"
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let updated: Value = r.json().await?;
    assert_eq!(updated["name"], "Acme Billing v2");
    assert_eq!(updated["description"], "Updated description");
    // git_remote should be unchanged
    assert_eq!(updated["git_remote"], "https://github.com/acme/billing");

    // 8. DELETE.
    let r = authed(
        req(
            &cl,
            reqwest::Method::DELETE,
            &h.base,
            "/v1/tenant/projects/acme-billing",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 204);

    // 9. GET after delete → 404.
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/acme-billing",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

/// Cross-tenant isolation: Globex cannot see Acme's projects.
#[tokio::test]
async fn projects_cross_tenant_isolation() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // Create a project under acme.
    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({"slug": "secret-project", "name": "Acme Secret"}))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 201);

    // Globex lists projects — should see zero.
    let list: Vec<Value> = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects",
            "globex",
        ),
        &h.globex_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(list.len(), 0, "globex must not see acme projects: {list:?}");

    // Globex trying to GET acme's project → 404 (not a 403, since the
    // project simply doesn't exist in their tenant).
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/secret-project",
            "globex",
        ),
        &h.globex_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

/// Non-admin token → 403 on all write routes and admin list/detail.
#[tokio::test]
async fn projects_requires_admin_scope() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // Reader tries to create.
    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_reader,
    )
    .json(&json!({"slug": "x", "name": "X"}))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 403);

    // Reader tries to list.
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_reader,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 403);

    Ok(())
}

/// Duplicate slug → 409 Conflict.
#[tokio::test]
async fn projects_duplicate_slug_is_conflict() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    let create = || {
        authed(
            req(
                &cl,
                reqwest::Method::POST,
                &h.base,
                "/v1/tenant/projects",
                "acme",
            ),
            &h.acme_admin,
        )
        .json(&json!({"slug": "dup", "name": "Duplicate"}))
    };

    let r = create().send().await?;
    assert_eq!(r.status().as_u16(), 201);

    let r = create().send().await?;
    assert_eq!(
        r.status().as_u16(),
        409,
        "second create with same slug must be 409"
    );

    Ok(())
}

/// Invalid item kind → 400.
#[tokio::test]
async fn projects_invalid_item_kind_is_bad_request() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // Create a project first.
    authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({"slug": "proj", "name": "Proj"}))
    .send()
    .await?;

    let r = authed(
        req(
            &cl,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/projects/proj/items",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!([{"slug": "axum", "kind": "tool"}]))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400, "bad kind should be 400");

    Ok(())
}

/// DELETE on non-existent project → 404.
#[tokio::test]
async fn projects_delete_nonexistent_is_404() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    let r = authed(
        req(
            &cl,
            reqwest::Method::DELETE,
            &h.base,
            "/v1/tenant/projects/ghost",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

/// `GET /v1/projects/resolve?remote=<url>` normalises SSH and HTTPS forms
/// and returns the project slug when found.
#[tokio::test]
async fn projects_resolve_by_remote_normalizes_urls() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // Create project with a normalized HTTPS remote.
    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "slug": "payment-svc",
        "name": "Payment Service",
        "git_remote": "https://github.com/acme/payment-svc"
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 201);

    // Resolve with an SSH variant — should match after normalization.
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/projects/resolve?remote=git%40github.com%3Aacme%2Fpayment-svc.git",
            "acme",
        ),
        &h.acme_reader, // any-member auth is sufficient
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "SSH remote should resolve: {}", {
        let txt = r.text().await?;
        txt
    });

    // Resolve with the exact HTTPS form (without .git).
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/projects/resolve?remote=https%3A%2F%2Fgithub.com%2Facme%2Fpayment-svc",
            "acme",
        ),
        &h.acme_reader,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await?;
    assert_eq!(body["slug"], "payment-svc");
    assert_eq!(body["name"], "Payment Service");

    // Resolve with .git suffix — should still match after stripping.
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/projects/resolve?remote=https%3A%2F%2Fgithub.com%2Facme%2Fpayment-svc.git",
            "acme",
        ),
        &h.acme_reader,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);

    // Unknown remote → 404.
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/projects/resolve?remote=https%3A%2F%2Fgithub.com%2Facme%2Funknown",
            "acme",
        ),
        &h.acme_reader,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

/// Resolve only works within the caller's tenant.
#[tokio::test]
async fn projects_resolve_cross_tenant_isolation() -> Result<()> {
    let h = boot().await?;
    let cl = c();

    // Create project under acme with a git remote.
    admin::create_project(
        &h.pool,
        "acme",
        "billing",
        "Billing",
        None,
        Some("https://github.com/acme/billing"),
    )
    .await?;

    // Globex tries to resolve acme's remote — should 404.
    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/projects/resolve?remote=https%3A%2F%2Fgithub.com%2Facme%2Fbilling",
            "globex",
        ),
        &h.globex_admin,
    )
    .send()
    .await?;
    assert_eq!(
        r.status().as_u16(),
        404,
        "globex must not resolve acme's remote"
    );

    Ok(())
}
