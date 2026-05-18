//! Integration test for the email-notification config surface.
//!
//! Real SMTP delivery requires a mock server and is out of scope for v1
//! — `send_email`'s lettre-driven path is exercised by unit tests on
//! body construction. What we cover here:
//!
//!   1. SMTP fields round-trip via PUT/GET.
//!   2. Partial updates leave untouched fields alone.
//!   3. Empty-string clears a field.
//!   4. Malformed `smtp_url` is rejected with 400.
//!   5. When SMTP is configured AND a draft is created with no webhook,
//!      the spawned task tries to deliver to SMTP and audit-logs the
//!      failure (we don't run a real relay, so it'll fail — that's the
//!      contract we want: it never blocks the draft response).

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

use skill_pool_server::{config, routes, state};

struct Harness {
    base: String,
    acme_admin: String,
    db: sqlx::PgPool,
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

    use skill_pool_server::admin;
    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    let acme_admin = admin::create_token(
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
        db: pool,
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

fn cl() -> reqwest::Client {
    reqwest::Client::builder().timeout(Duration::from_secs(15)).build().unwrap()
}
fn req(c: &reqwest::Client, m: reqwest::Method, b: &str, p: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{b}{p}")).header("x-skill-pool-tenant", "acme")
}
fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

#[tokio::test]
async fn email_config_round_trip_and_failed_delivery_is_audit_logged() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // 1. PUT SMTP-only config.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    )
    .json(&json!({
        "smtp_url": "smtp://user:pass@127.0.0.1:2525",
        "smtp_from": "skill-pool <noreply@example.com>",
        "smtp_to": "curators@example.com",
    }))
    .send().await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let body: Value = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(body["smtp_url"], "smtp://user:pass@127.0.0.1:2525");
    assert_eq!(body["smtp_from"], "skill-pool <noreply@example.com>");
    assert_eq!(body["smtp_to"], "curators@example.com");
    // Webhook still unset.
    assert!(body.get("webhook_url").is_none() || body["webhook_url"].is_null());

    // 2. Partial — change only smtp_to.
    let _ = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    )
    .json(&json!({ "smtp_to": "different@example.com" }))
    .send().await?;
    let body: Value = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert_eq!(body["smtp_to"], "different@example.com");
    // Untouched fields preserved.
    assert_eq!(body["smtp_url"], "smtp://user:pass@127.0.0.1:2525");
    assert_eq!(body["smtp_from"], "skill-pool <noreply@example.com>");

    // 3. Empty string clears.
    let _ = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    )
    .json(&json!({ "smtp_to": "" }))
    .send().await?;
    let body: Value = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    ).send().await?.json().await?;
    assert!(body.get("smtp_to").is_none_or(|v| v.is_null()));

    // Restore for the rest of the test.
    let _ = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    )
    .json(&json!({ "smtp_to": "curators@example.com" }))
    .send().await?;

    // 4. Malformed smtp_url → 400.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/notifications"),
        &h.acme_admin,
    )
    .json(&json!({ "smtp_url": "ftp://nope" }))
    .send().await?;
    assert_eq!(r.status().as_u16(), 400);

    // 5. Create a draft — the spawned task tries to deliver, fails (no
    //    SMTP relay running on 127.0.0.1:2525), and writes a 'failed'
    //    audit row. We wait briefly then check the audit_events table.
    let bundle = build_bundle("---\nname: foo\ndescription: A test.\n---\n\nbody\n");
    let meta = json!({ "slug": "foo", "origin": "cli" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec()).file_name("foo.tar.gz").mime_str("application/gzip")?,
    );
    let r = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/drafts"),
        &h.acme_admin,
    )
    .multipart(form).send().await?;
    assert_eq!(r.status().as_u16(), 201, "draft create blocked on email: {}", r.text().await?);

    // Poll the audit log for the email delivery row (best-effort, up to 5s).
    let mut attempts = 0;
    let audit_row: Option<(String, Value)> = loop {
        let row: Option<(String, Value)> = sqlx::query_as(
            "SELECT action, metadata FROM audit_events \
             WHERE action = 'notification.deliver' AND target_kind = 'email' \
             ORDER BY ts DESC LIMIT 1",
        )
        .fetch_optional(&h.db)
        .await?;
        if row.is_some() || attempts > 50 {
            break row;
        }
        attempts += 1;
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let (action, metadata) = audit_row.expect("expected an email notification.deliver audit row");
    assert_eq!(action, "notification.deliver");
    // Failed delivery (no real relay) — but the contract is "audit row gets written."
    assert_eq!(metadata["result"], "failed");
    assert_eq!(metadata["to"], "curators@example.com");

    Ok(())
}
