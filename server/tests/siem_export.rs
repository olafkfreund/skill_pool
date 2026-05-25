//! Phase 5 integration test: tenant-configurable SIEM export.
//!
//! Spins up:
//!   - skill-pool server (testcontainer postgres + the real routes)
//!   - a tiny axum receiver on a random port that records every POST,
//!     including the `Authorization` header so we can check the bearer
//!     token flow that Splunk HEC / Datadog Logs both use.
//!
//! Coverage:
//!   1. PUT /v1/tenant/audit-siem sets the URL → GET reflects it,
//!      and the token is redacted to a boolean indicator.
//!   2. Any audit write (here: triggered by a draft publish via the
//!      tenant theme PUT, which is an admin-scoped mutating endpoint
//!      that calls `audit::record_best_effort`) fans out to the SIEM
//!      receiver with the canonical payload shape.
//!   3. Bearer token is sent as `Authorization: Bearer <token>`.
//!   4. When no SIEM is configured, audit writes don't fire any POST.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

/// Records every POST the stub receives, including the bearer token
/// (when present) so the test can verify the auth-header convention.
#[derive(Default, Clone)]
struct SiemReceiver {
    pub calls: Arc<Mutex<Vec<ReceivedCall>>>,
}

#[derive(Clone, Debug)]
struct ReceivedCall {
    authorization: Option<String>,
    body: Vec<u8>,
}

async fn siem_handler(
    State(rx): State<SiemReceiver>,
    headers: HeaderMap,
    body: Bytes,
) -> &'static str {
    let authorization = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    rx.calls.lock().unwrap().push(ReceivedCall {
        authorization,
        body: body.to_vec(),
    });
    "ok"
}

async fn boot_siem_receiver() -> Result<(String, SiemReceiver)> {
    let rx = SiemReceiver::default();
    let app = Router::new()
        .route("/", post(siem_handler))
        .with_state(rx.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    Ok((format!("http://{addr}/"), rx))
}

struct Harness {
    base: String,
    acme_admin_token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

async fn boot() -> Result<Harness> {
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

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme Corp", "team").await?;
    // tenant:admin scope covers both the SIEM config endpoint and the
    // theme-PUT we use to trigger an audit write below.
    let acme_admin_token = admin::create_token(&pool, "acme", "admin", "tenant:admin")
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
        acme_admin_token,
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

/// Hits `PUT /v1/tenant/audit-siem` as a deterministic audit trigger —
/// success path calls `audit::record_best_effort` with the well-known
/// `tenant.audit_siem.update` action. The actual fan-out works the
/// same way for any audited write; using the SIEM PUT keeps the test
/// hermetic (no bundle upload, no extra DB fixtures).
async fn trigger_audit_event(c: &reqwest::Client, h: &Harness) -> Result<()> {
    let resp = authed(
        req(
            c,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/audit-siem",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!({}))
    .send()
    .await?;
    let status = resp.status().as_u16();
    assert!(status == 200, "audit-siem PUT failed: status={status}");
    Ok(())
}

/// Wait up to ~3s for the receiver to record at least `n` calls.
async fn wait_for_calls(rx: &SiemReceiver, n: usize) -> Result<()> {
    for _ in 0..30 {
        if rx.calls.lock().unwrap().len() >= n {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "timed out waiting for {n} SIEM POSTs (got {})",
        rx.calls.lock().unwrap().len()
    )
}

#[tokio::test]
async fn put_and_get_audit_siem_config_redacts_token() -> Result<()> {
    let h = boot().await?;
    let c = client();

    // Set URL + token in one PUT.
    let resp = authed(
        req(
            &c,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/audit-siem",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!({
        "url": "https://siem.example.com/services/collector",
        "token": "splunk-hec-token-shhh",
    }))
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["url"], "https://siem.example.com/services/collector");
    assert_eq!(body["token_configured"], true);
    // Token must never come back over the wire.
    assert!(
        body.as_object().is_some_and(|m| !m.contains_key("token")),
        "token must be redacted from response: {body}"
    );

    // GET reflects the saved config with the same redaction rule.
    let body: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/audit-siem",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(body["url"], "https://siem.example.com/services/collector");
    assert_eq!(body["token_configured"], true);
    Ok(())
}

#[tokio::test]
async fn unconfigured_siem_reports_null_url_and_false_token() -> Result<()> {
    let h = boot().await?;
    let c = client();

    let body: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/audit-siem",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert!(body["url"].is_null(), "url should be null: {body}");
    assert_eq!(body["token_configured"], false);
    Ok(())
}

#[tokio::test]
async fn audit_event_fans_out_to_siem_with_bearer_token() -> Result<()> {
    let h = boot().await?;
    let (siem_url, rx) = boot_siem_receiver().await?;
    let c = client();

    // Configure the SIEM destination.
    let token = "splunk-hec-AAAA-BBBB";
    let resp = authed(
        req(
            &c,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/audit-siem",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!({ "url": siem_url, "token": token }))
    .send()
    .await?;
    assert_eq!(resp.status().as_u16(), 200);

    // The PUT itself triggered a `tenant.audit_siem.update` audit row,
    // so the receiver may already see it. Drop whatever is queued so we
    // can assert on the next, deterministic trigger.
    wait_for_calls(&rx, 1).await?;
    rx.calls.lock().unwrap().clear();

    // Now do the actual trigger.
    trigger_audit_event(&c, &h).await?;
    wait_for_calls(&rx, 1).await?;

    let call = rx.calls.lock().unwrap()[0].clone();
    assert_eq!(
        call.authorization.as_deref(),
        Some(&*format!("Bearer {token}")),
        "expected Bearer token header"
    );

    let payload: Value = serde_json::from_slice(&call.body)?;
    // Schema mirrors the audit_events row 1:1.
    assert!(payload["id"].is_i64(), "id must be a bigint: {payload}");
    assert!(
        payload["tenant_id"].is_string(),
        "tenant_id must be uuid string: {payload}"
    );
    assert_eq!(payload["action"], "tenant.audit_siem.update");
    assert_eq!(payload["target_kind"], "tenant");
    assert!(payload["metadata"].is_object());
    assert!(
        payload["ts"]
            .as_str()
            .is_some_and(|s| s.contains('T') && s.ends_with('Z') || s.contains('+')),
        "ts must be ISO-8601: {payload}"
    );
    Ok(())
}

#[tokio::test]
async fn no_siem_configured_is_silent_no_op() -> Result<()> {
    let h = boot().await?;
    let (_siem_url, rx) = boot_siem_receiver().await?;
    let c = client();

    // Don't configure the SIEM — just trigger an audit event.
    trigger_audit_event(&c, &h).await?;

    // Give the spawned task a moment to NOT fire.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        rx.calls.lock().unwrap().is_empty(),
        "no SIEM should fire when none is configured (got {} calls)",
        rx.calls.lock().unwrap().len(),
    );
    Ok(())
}
