//! Integration tests for Project Plans REST API.
//!
//! Covers:
//!  1. Import via body_md → GET active plan shows it → list_versions returns 1.
//!  2. Re-import the same content → dedup → still 1 version.
//!  3. Import different content → 2 versions; latest is active.
//!  4. activate v1 → v1 is active; v2 is superseded.
//!  5. GET /plan/versions/{v} returns the specific version.
//!  6. Import file source_type with source_url provenance.
//!  7. Refresh with no source_url → outcome: "unchanged".
//!  8. Non-admin token → 403 on import.
//!  9. Unknown project → 404.
//! 10. Import with empty body → 400.

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

    // Create a project for the tests to use.
    admin::create_project(&pool, "acme", "billing", "Billing Service", None, None).await?;

    let acme_admin = admin::create_token(&pool, "acme", "admin-tok", "tenant:admin skills:read")
        .await?
        .raw_token;
    let acme_reader = admin::create_token(&pool, "acme", "reader-tok", "skills:read")
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
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn req(
    cl: &reqwest::Client,
    method: reqwest::Method,
    base: &str,
    path: &str,
    tenant: &str,
) -> reqwest::RequestBuilder {
    cl.request(method, format!("{base}{path}"))
        .header("x-skill-pool-tenant", tenant)
}

fn authed(b: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(token)
}

// ---------------------------------------------------------------------------
// Test 1-6: core plan lifecycle
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_import_versions_activate_lifecycle() -> Result<()> {
    let h = boot().await?;
    let cl = client();

    let plan_v1_body = "# Billing Service Plan\n\nThis is v1 of the plan.\n";
    let plan_v2_body = "# Billing Service Plan\n\nThis is v2 of the plan.\n";

    // --- Test 1: import via body_md ---
    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "source_type": "file",
        "source_url": "/local/path/plan.md",
        "body_md": plan_v1_body
    }))
    .send()
    .await?;
    assert_eq!(
        r.status().as_u16(),
        201,
        "import failed: {}",
        r.text().await?
    );
    let plan: Value = r.json().await?;
    assert_eq!(plan["version"], 1, "first import should be version 1");
    assert_eq!(plan["status"], "active");
    assert_eq!(plan["source_type"], "file");
    let v1_id = plan["id"].as_str().unwrap().to_owned();

    // --- GET active plan returns it ---
    let active: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(active["version"], 1);
    assert_eq!(active["body_md"].as_str().unwrap(), plan_v1_body);

    // --- list_versions returns exactly 1 ---
    let versions: Vec<Value> = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan/versions",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(versions.len(), 1, "expected 1 version: {versions:?}");
    // body_md is absent from the slim listing
    assert!(
        versions[0].get("body_md").is_none(),
        "slim listing must omit body_md"
    );

    // --- Test 2: re-import same content → dedup → still 1 version ---
    let r2 = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "source_type": "file",
        "body_md": plan_v1_body
    }))
    .send()
    .await?;
    assert_eq!(
        r2.status().as_u16(),
        201,
        "dedup import failed: {}",
        r2.text().await?
    );
    let deduped: Value = r2.json().await?;
    // Must return the SAME version id (no new row created).
    assert_eq!(
        deduped["id"].as_str().unwrap(),
        v1_id,
        "dedup should return existing plan row"
    );

    let versions_after_dedup: Vec<Value> = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan/versions",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(
        versions_after_dedup.len(),
        1,
        "dedup should not create a new version"
    );

    // --- Test 3: import different content → 2 versions, latest active ---
    let r3 = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "source_type": "file",
        "body_md": plan_v2_body
    }))
    .send()
    .await?;
    assert_eq!(
        r3.status().as_u16(),
        201,
        "v2 import failed: {}",
        r3.text().await?
    );
    let plan_v2: Value = r3.json().await?;
    assert_eq!(
        plan_v2["version"], 2,
        "second unique import should be version 2"
    );
    assert_eq!(plan_v2["status"], "active");

    // Active plan should now be v2.
    let active_v2: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(active_v2["version"], 2);

    let versions_after_v2: Vec<Value> = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan/versions",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(
        versions_after_v2.len(),
        2,
        "should have 2 versions: {versions_after_v2:?}"
    );

    // --- Test 4: activate v1 → v1 is active, v2 is superseded ---
    let r4 = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan/activate",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({ "version": 1 }))
    .send()
    .await?;
    assert_eq!(
        r4.status().as_u16(),
        200,
        "activate failed: {}",
        r4.text().await?
    );
    let activated: Value = r4.json().await?;
    assert_eq!(activated["version"], 1);
    assert_eq!(activated["status"], "active");

    // GET active should now be v1.
    let re_active: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(
        re_active["version"], 1,
        "after activate v1, active should be v1"
    );

    // --- Test 5: GET /plan/versions/{v} returns specific version with body ---
    let specific: Value = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/billing/plan/versions/2",
            "acme",
        ),
        &h.acme_reader,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(specific["version"], 2);
    assert_eq!(
        specific["body_md"].as_str().unwrap(),
        plan_v2_body,
        "specific version endpoint must include body_md"
    );

    // --- Test 6: import with file source_type + explicit source_url provenance ---
    let r6 = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "source_type": "file",
        "source_url": "/home/user/docs/plan.md",
        "body_md": "# v3 from file with provenance\n"
    }))
    .send()
    .await?;
    assert_eq!(
        r6.status().as_u16(),
        201,
        "file+url import failed: {}",
        r6.text().await?
    );
    let plan_v3: Value = r6.json().await?;
    assert_eq!(plan_v3["source_type"], "file");
    assert_eq!(plan_v3["source_url"], "/home/user/docs/plan.md");

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 7: refresh with no source_url → unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_refresh_no_source_url_unchanged() -> Result<()> {
    let h = boot().await?;
    let cl = client();

    // Import a file-sourced plan (no URL to re-fetch).
    authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "source_type": "file",
        "body_md": "# Static plan\n"
    }))
    .send()
    .await?;

    // Refresh should be a no-op.
    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan/refresh",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(
        r.status().as_u16(),
        200,
        "refresh failed: {}",
        r.text().await?
    );
    let resp: Value = r.json().await?;
    assert_eq!(
        resp["outcome"].as_str().unwrap(),
        "unchanged",
        "refresh with no source_url must return outcome=unchanged"
    );
    assert!(
        resp["version"].is_null(),
        "version should be null for unchanged outcome"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 8: non-admin token → 403 on write routes
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_import_requires_admin() -> Result<()> {
    let h = boot().await?;
    let cl = client();

    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_reader,
    )
    .json(&json!({
        "source_type": "file",
        "body_md": "# Not allowed\n"
    }))
    .send()
    .await?;
    assert_eq!(
        r.status().as_u16(),
        403,
        "reader should not be able to import plans: {}",
        r.text().await?
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 9: unknown project → 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_unknown_project_404() -> Result<()> {
    let h = boot().await?;
    let cl = client();

    let r = authed(
        req(
            &cl,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/projects/no-such-project/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .send()
    .await?;
    // The project doesn't exist → admin fn returns not-found → 404
    assert_eq!(
        r.status().as_u16(),
        404,
        "missing project should 404: {}",
        r.text().await?
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// Test 10: empty body → 400
// ---------------------------------------------------------------------------

#[tokio::test]
async fn plan_empty_body_400() -> Result<()> {
    let h = boot().await?;
    let cl = client();

    let r = authed(
        req(
            &cl,
            reqwest::Method::POST,
            &h.base,
            "/v1/tenant/projects/billing/plan",
            "acme",
        ),
        &h.acme_admin,
    )
    .json(&json!({
        "source_type": "file",
        "body_md": ""
    }))
    .send()
    .await?;
    assert_eq!(
        r.status().as_u16(),
        400,
        "empty body_md should return 400: {}",
        r.text().await?
    );

    Ok(())
}
