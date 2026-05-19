//! Phase 5 integration test: curator webhook delivery.
//!
//! Spins up:
//!   - skill-pool server (testcontainer postgres + the real routes)
//!   - a tiny axum receiver on a random port to play the role of the
//!     team's Slack/Discord/custom webhook endpoint.
//!
//! Coverage:
//!   1. PUT /v1/tenant/notifications sets the URL → GET reflects it.
//!   2. POST /v1/drafts → receiver gets a POST with the expected event
//!      and a Slack-compatible `text` field, within a few seconds.
//!   3. When a secret is configured, the request carries an HMAC
//!      `X-Skill-Pool-Signature` header that verifies against the body.
//!   4. /v1/tenant/notifications/pending-count returns the right number.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::routing::post;
use axum::Router;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use hmac::{Hmac, Mac};
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use std::sync::Mutex;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

/// Records every webhook the stub receives. Wrapped in `Arc<Mutex<>>` so
/// the axum handler can push without ceremony.
#[derive(Default, Clone)]
struct WebhookReceiver {
    pub calls: Arc<Mutex<Vec<ReceivedCall>>>,
}

#[derive(Clone, Debug)]
struct ReceivedCall {
    signature: Option<String>,
    body: Vec<u8>,
}

async fn webhook_handler(
    State(rx): State<WebhookReceiver>,
    headers: HeaderMap,
    body: Bytes,
) -> &'static str {
    let signature = headers
        .get("x-skill-pool-signature")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    rx.calls.lock().unwrap().push(ReceivedCall {
        signature,
        body: body.to_vec(),
    });
    "ok"
}

async fn boot_webhook_receiver() -> Result<(String, WebhookReceiver)> {
    let rx = WebhookReceiver::default();
    let app = Router::new()
        .route("/", post(webhook_handler))
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
    // tenant:admin scope is required for the PUT config endpoint.
    let acme_admin_token = admin::create_token(
        &pool,
        "acme",
        "admin",
        "tenant:admin skills:read skills:publish",
    )
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

async fn create_draft(c: &reqwest::Client, h: &Harness, slug: &str) -> Result<Value> {
    let bundle = build_bundle(&format!(
        "---\nname: {slug}\ndescription: Pattern about {slug}.\ntags: [test]\n---\n\n# {slug}\n"
    ));
    let meta = json!({ "slug": slug, "origin": "cli" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec())
            .file_name(format!("{slug}.tar.gz"))
            .mime_str("application/gzip")?,
    );
    let resp = authed(
        req(c, reqwest::Method::POST, &h.base, "/v1/drafts", "acme"),
        &h.acme_admin_token,
    )
    .multipart(form)
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 201, "{body}");
    Ok(body)
}

/// Wait up to ~3 seconds for the receiver to record at least `n` calls.
/// Used because webhook delivery is `tokio::spawn`'d and may land after the
/// drafts POST returns.
async fn wait_for_calls(rx: &WebhookReceiver, n: usize) -> Result<()> {
    for _ in 0..30 {
        if rx.calls.lock().unwrap().len() >= n {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    anyhow::bail!(
        "timed out waiting for {n} webhook calls (got {})",
        rx.calls.lock().unwrap().len()
    )
}

#[tokio::test]
async fn webhook_fires_on_draft_create_and_carries_text_field() -> Result<()> {
    let h = boot().await?;
    let (webhook_url, rx) = boot_webhook_receiver().await?;
    let c = client();

    // 1. Configure the webhook (no secret).
    let resp = authed(
        req(
            &c,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/notifications",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!({ "webhook_url": webhook_url }))
    .send()
    .await?;
    let status = resp.status().as_u16();
    let body: Value = resp.json().await?;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["webhook_url"], webhook_url);
    assert_eq!(body["signing_enabled"], false);

    // 2. GET reflects what we set.
    let body: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/notifications",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(body["webhook_url"], webhook_url);

    // 3. Create a draft → receiver gets the POST.
    create_draft(&c, &h, "axum-handler-tip").await?;
    wait_for_calls(&rx, 1).await?;

    let call = {
        let calls = rx.calls.lock().unwrap();
        calls[0].clone()
    };
    assert!(call.signature.is_none(), "no secret → no signature header");
    let payload: Value = serde_json::from_slice(&call.body)?;
    assert_eq!(payload["event"], "draft.created");
    assert_eq!(payload["tenant"]["slug"], "acme");
    assert_eq!(payload["draft"]["slug"], "axum-handler-tip");
    assert!(payload["text"]
        .as_str()
        .unwrap_or_default()
        .contains("axum-handler-tip"));

    // 4. pending-count reflects the unreviewed draft.
    let count: Value = authed(
        req(
            &c,
            reqwest::Method::GET,
            &h.base,
            "/v1/tenant/notifications/pending-count",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .send()
    .await?
    .json()
    .await?;
    assert_eq!(count["pending"], 1);

    Ok(())
}

#[tokio::test]
async fn webhook_signs_body_when_secret_configured() -> Result<()> {
    let h = boot().await?;
    let (webhook_url, rx) = boot_webhook_receiver().await?;
    let c = client();

    // Configure URL + secret in one PUT.
    let secret = "supersecret-12345";
    let resp = authed(
        req(
            &c,
            reqwest::Method::PUT,
            &h.base,
            "/v1/tenant/notifications",
            "acme",
        ),
        &h.acme_admin_token,
    )
    .json(&json!({
        "webhook_url": webhook_url,
        "webhook_secret": secret,
    }))
    .send()
    .await?;
    let body: Value = resp.json().await?;
    assert_eq!(body["signing_enabled"], true);

    create_draft(&c, &h, "signed-tip").await?;
    wait_for_calls(&rx, 1).await?;

    let call = rx.calls.lock().unwrap()[0].clone();
    let sig = call.signature.expect("expected signature header");
    let expected = {
        let mut mac =
            Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
        mac.update(&call.body);
        format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
    };
    assert_eq!(sig, expected, "signature should match HMAC over body");

    Ok(())
}

#[tokio::test]
async fn no_webhook_configured_is_silent_no_op() -> Result<()> {
    let h = boot().await?;
    let (_webhook_url, rx) = boot_webhook_receiver().await?;
    let c = client();

    // Create a draft without ever setting a webhook.
    create_draft(&c, &h, "lonely-tip").await?;

    // Give the spawned task a moment to NOT fire.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        rx.calls.lock().unwrap().is_empty(),
        "no webhook should fire when none is configured"
    );
    Ok(())
}
