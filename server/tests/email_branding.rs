//! Integration test for per-tenant branded transactional email (#9).
//!
//! We don't run a real SMTP relay — the send path is exercised by
//! lower-level unit tests in `email_branding::tests` and a separate
//! `email_notifications.rs` suite that already covers the delivery
//! audit-row contract. What this suite covers is the *config surface*:
//!
//!   1. PUT branding → 200, password NOT echoed back; GET returns
//!      `password_configured: true`.
//!   2. The DB row's `smtp_password_enc` column is NOT the plaintext
//!      password (verifies at-rest encryption / fallback round-tripped
//!      through the encrypt path, not stored raw).
//!   3. DELETE → 204; subsequent GET → 404.
//!   4. Validation: malformed `from_addr` and bad scheme are rejected
//!      at the API layer so admins can't write garbage rows that the
//!      send path would later refuse.

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
    db: sqlx::PgPool,
    tenant_id: uuid::Uuid,
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

    let (tenant_id,): (uuid::Uuid,) =
        sqlx::query_as("SELECT id FROM tenants WHERE slug = 'acme'")
            .fetch_one(&pool)
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
        tenant_id,
        _pg: pg,
        _storage_dir: storage_dir,
    })
}

fn cl() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .unwrap()
}

fn req(c: &reqwest::Client, m: reqwest::Method, base: &str, path: &str) -> reqwest::RequestBuilder {
    c.request(m, format!("{base}{path}"))
        .header("x-skill-pool-tenant", "acme")
}

fn authed(b: reqwest::RequestBuilder, t: &str) -> reqwest::RequestBuilder {
    b.bearer_auth(t)
}

#[tokio::test]
async fn email_branding_round_trip_encrypts_and_masks_password() -> Result<()> {
    // Set a deterministic encryption key so the test exercises the
    // AES-GCM path rather than the base64 fallback.
    std::env::set_var(
        skill_pool_server::email_branding::ENCRYPTION_KEY_ENV,
        "0".repeat(64),
    );

    let h = boot().await?;
    let c = cl();

    // 1. Initial GET → 404 (no row yet).
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/email-branding"),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    // 2. PUT branding — full payload.
    let password = "super-secret-smtp-password";
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/email-branding"),
        &h.acme_admin,
    )
    .json(&json!({
        "from_addr": "noreply@acme.example.com",
        "from_name": "Acme Skill Pool",
        "reply_to": "support@acme.example.com",
        "smtp_url": "smtps://relay@smtp.acme.example.com:465",
        "smtp_password": password,
        "footer_html": "Acme Corp — internal communications",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let body: Value = r.json().await?;
    assert_eq!(body["from_addr"], "noreply@acme.example.com");
    assert_eq!(body["from_name"], "Acme Skill Pool");
    assert_eq!(body["password_configured"], true);
    // Most important: password NEVER appears in the response.
    let body_str = serde_json::to_string(&body)?;
    assert!(
        !body_str.contains(password),
        "PUT response leaked the plaintext password: {body_str}"
    );

    // 3. GET → 200, same shape, still no password.
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/email-branding"),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await?;
    assert_eq!(body["password_configured"], true);
    assert_eq!(body["reply_to"], "support@acme.example.com");
    assert_eq!(body["footer_html"], "Acme Corp — internal communications");
    let body_str = serde_json::to_string(&body)?;
    assert!(!body_str.contains(password), "GET leaked password");

    // 4. Verify the stored ciphertext is not the plaintext.
    let (enc,): (Vec<u8>,) = sqlx::query_as(
        "SELECT smtp_password_enc FROM tenant_email_branding WHERE tenant_id = $1",
    )
    .bind(h.tenant_id)
    .fetch_one(&h.db)
    .await?;
    assert!(!enc.is_empty());
    // The plaintext bytes must not appear anywhere in the stored blob.
    let pt_bytes = password.as_bytes();
    let leaked = enc.windows(pt_bytes.len()).any(|w| w == pt_bytes);
    assert!(!leaked, "stored blob contains plaintext password bytes");
    // Format byte should be the AES-GCM marker (0x01) since we set the key.
    assert_eq!(enc[0], 0x01, "expected AES-GCM format byte");

    // 5. Validation — bad from_addr → 400.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/email-branding"),
        &h.acme_admin,
    )
    .json(&json!({
        "from_addr": "not-an-email",
        "smtp_url": "smtps://relay@smtp.acme.example.com:465",
        "smtp_password": "x",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400);

    // 6. Validation — bad scheme → 400.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/email-branding"),
        &h.acme_admin,
    )
    .json(&json!({
        "from_addr": "noreply@acme.example.com",
        "smtp_url": "ftp://nope",
        "smtp_password": "x",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400);

    // 7. /test with no recipient validity → 400.
    let r = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/tenant/email-branding/test"),
        &h.acme_admin,
    )
    .json(&json!({ "recipient": "not-an-email" }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400);

    // 8. /test with a valid recipient succeeds at the API layer (the
    //    actual SMTP send will fail because no relay is running — we
    //    just need the 200 + structured failure result).
    let r = authed(
        req(&c, reqwest::Method::POST, &h.base, "/v1/tenant/email-branding/test"),
        &h.acme_admin,
    )
    .json(&json!({ "recipient": "ops@acme.example.com" }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let body: Value = r.json().await?;
    // No relay on the configured host — expect a structured failure.
    assert_eq!(body["result"], "failed");
    assert!(body.get("error").is_some());

    // 9. DELETE → 204; subsequent GET → 404.
    let r = authed(
        req(
            &c,
            reqwest::Method::DELETE,
            &h.base,
            "/v1/tenant/email-branding",
        ),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 204);
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/email-branding"),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

#[tokio::test]
async fn non_admin_caller_is_forbidden() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // Mint a non-admin token (skills scope only).
    use skill_pool_server::admin;
    let viewer = admin::create_token(&h.db, "acme", "viewer", "skills:read")
        .await?
        .raw_token;

    for path in [
        "/v1/tenant/email-branding",
    ] {
        let r = authed(req(&c, reqwest::Method::GET, &h.base, path), &viewer)
            .send()
            .await?;
        assert_eq!(r.status().as_u16(), 403, "GET {path}");
        let r = authed(req(&c, reqwest::Method::DELETE, &h.base, path), &viewer)
            .send()
            .await?;
        assert_eq!(r.status().as_u16(), 403, "DELETE {path}");
    }

    Ok(())
}
