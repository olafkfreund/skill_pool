//! Integration tests for the admin SSO config surface (#4).
//!
//! Coverage:
//!   1. OIDC PUT → 200; GET returns masked `client_secret_hint`; DB row
//!      stores the raw secret so the runtime exchange path keeps working.
//!   2. SAML PUT (with realistic IdP metadata XML) → 200; GET reflects
//!      the parsed entity ID, SSO URL, and cert byte count.
//!   3. DELETE clears both rows; subsequent GET → 200 with `kind: null`.
//!   4. Validation: bad URL / role / XML → 400.
//!   5. Non-admin scope → 403 on every verb.

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

// Minimal IdP metadata document — `parse_saml_metadata` accepts this shape.
fn sample_idp_metadata() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<EntityDescriptor xmlns="urn:oasis:names:tc:SAML:2.0:metadata"
                  xmlns:ds="http://www.w3.org/2000/09/xmldsig#"
                  entityID="https://idp.example.com/saml2/idp/metadata.php">
  <IDPSSODescriptor protocolSupportEnumeration="urn:oasis:names:tc:SAML:2.0:protocol">
    <KeyDescriptor use="signing">
      <ds:KeyInfo>
        <ds:X509Data>
          <ds:X509Certificate>MIIBszCCARygAwIBAgIJAKxQfakeCertificateBytesHere</ds:X509Certificate>
        </ds:X509Data>
      </ds:KeyInfo>
    </KeyDescriptor>
    <SingleSignOnService Binding="urn:oasis:names:tc:SAML:2.0:bindings:HTTP-Redirect"
                         Location="https://idp.example.com/saml2/idp/SSOService.php"/>
  </IDPSSODescriptor>
</EntityDescriptor>"#
}

#[tokio::test]
async fn oidc_round_trip_masks_secret_and_keeps_plaintext_in_db() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // 1. Initial GET → 200 with kind: null (no rows yet).
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/sso"),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await?;
    assert!(body["kind"].is_null());
    assert_eq!(body["scim_endpoint"], "/scim/v2/Users");

    // 2. PUT OIDC.
    let secret = "spk-client-secret-AAAA1234";
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/oidc"),
        &h.acme_admin,
    )
    .json(&json!({
        "issuer_url": "https://login.example.com/realms/acme",
        "client_id": "skill-pool-spk",
        "client_secret": secret,
        "default_role": "publisher",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let body: Value = r.json().await?;
    assert_eq!(body["kind"], "oidc");
    assert_eq!(body["oidc"]["issuer_url"], "https://login.example.com/realms/acme");
    assert_eq!(body["oidc"]["client_id"], "skill-pool-spk");
    assert_eq!(body["oidc"]["default_role"], "publisher");
    assert_eq!(body["oidc"]["client_secret_hint"], "••••1234");
    // The plaintext must NEVER appear in the response.
    let body_str = serde_json::to_string(&body)?;
    assert!(
        !body_str.contains(secret),
        "PUT response leaked plaintext: {body_str}"
    );

    // 3. GET — same masking, same shape.
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/sso"),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await?;
    assert_eq!(body["kind"], "oidc");
    assert_eq!(body["oidc"]["client_secret_hint"], "••••1234");
    let body_str = serde_json::to_string(&body)?;
    assert!(!body_str.contains(secret), "GET leaked secret");

    // 4. The DB still holds the plaintext — the runtime OIDC code path
    //    needs it verbatim to exchange the auth code. (We deliberately
    //    don't encrypt this column today; the bar is "don't echo it
    //    over the wire", which the API masks.)
    let (stored,): (String,) =
        sqlx::query_as("SELECT client_secret FROM tenant_sso WHERE tenant_id = $1")
            .bind(h.tenant_id)
            .fetch_one(&h.db)
            .await?;
    assert_eq!(stored, secret);

    // 5. PUT again with a new secret — upserts in place.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/oidc"),
        &h.acme_admin,
    )
    .json(&json!({
        "issuer_url": "https://login.example.com/realms/acme",
        "client_id": "skill-pool-spk",
        "client_secret": "new-secret-XYZ9999",
        "default_role": "admin",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200);
    let body: Value = r.json().await?;
    assert_eq!(body["oidc"]["client_secret_hint"], "••••9999");
    assert_eq!(body["oidc"]["default_role"], "admin");

    // 6. DELETE → 204; subsequent GET → 200 with kind: null.
    let r = authed(
        req(&c, reqwest::Method::DELETE, &h.base, "/v1/tenant/sso"),
        &h.acme_admin,
    )
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 204);
    let r = authed(
        req(&c, reqwest::Method::GET, &h.base, "/v1/tenant/sso"),
        &h.acme_admin,
    )
    .send()
    .await?;
    let body: Value = r.json().await?;
    assert!(body["kind"].is_null());
    assert!(body.get("oidc").is_none() || body["oidc"].is_null());

    Ok(())
}

#[tokio::test]
async fn saml_put_parses_metadata_and_persists() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/saml"),
        &h.acme_admin,
    )
    .json(&json!({
        "metadata_xml": sample_idp_metadata(),
        "default_role": "viewer",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let body: Value = r.json().await?;
    assert_eq!(body["kind"], "saml");
    assert_eq!(
        body["saml"]["idp_entity_id"],
        "https://idp.example.com/saml2/idp/metadata.php"
    );
    assert_eq!(
        body["saml"]["idp_sso_url"],
        "https://idp.example.com/saml2/idp/SSOService.php"
    );
    assert_eq!(body["saml"]["default_role"], "viewer");
    let cert_bytes = body["saml"]["idp_x509_cert_bytes"].as_u64().unwrap();
    assert!(cert_bytes > 80, "expected PEM-wrapped cert to be > 80 bytes");

    // Stored cert is PEM-wrapped, ready for the runtime SAML path.
    let (cert,): (String,) =
        sqlx::query_as("SELECT idp_x509_cert FROM tenant_saml WHERE tenant_id = $1")
            .bind(h.tenant_id)
            .fetch_one(&h.db)
            .await?;
    assert!(cert.starts_with("-----BEGIN CERTIFICATE-----"));
    assert!(cert.contains("MIIBszCCARygAwIBAgIJAKxQfakeCertificateBytesHere"));
    assert!(cert.trim_end().ends_with("-----END CERTIFICATE-----"));

    Ok(())
}

#[tokio::test]
async fn rejects_garbage_inputs() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // OIDC: bad URL.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/oidc"),
        &h.acme_admin,
    )
    .json(&json!({
        "issuer_url": "not a url",
        "client_id": "x",
        "client_secret": "y",
        "default_role": "viewer",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400);

    // OIDC: bad role.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/oidc"),
        &h.acme_admin,
    )
    .json(&json!({
        "issuer_url": "https://example.com/",
        "client_id": "x",
        "client_secret": "y",
        "default_role": "operator",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400);

    // SAML: garbage XML.
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/saml"),
        &h.acme_admin,
    )
    .json(&json!({
        "metadata_xml": "definitely not xml",
        "default_role": "viewer",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 400);

    Ok(())
}

#[tokio::test]
async fn non_admin_caller_is_forbidden() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    use skill_pool_server::admin;
    let viewer = admin::create_token(&h.db, "acme", "viewer", "skills:read")
        .await?
        .raw_token;

    // GET, DELETE on /v1/tenant/sso
    for m in [reqwest::Method::GET, reqwest::Method::DELETE] {
        let r = authed(req(&c, m.clone(), &h.base, "/v1/tenant/sso"), &viewer)
            .send()
            .await?;
        assert_eq!(r.status().as_u16(), 403, "{} /v1/tenant/sso", m);
    }

    // PUT /oidc, PUT /saml
    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/oidc"),
        &viewer,
    )
    .json(&json!({
        "issuer_url": "https://example.com/",
        "client_id": "x",
        "client_secret": "y",
        "default_role": "viewer",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 403);

    let r = authed(
        req(&c, reqwest::Method::PUT, &h.base, "/v1/tenant/sso/saml"),
        &viewer,
    )
    .json(&json!({
        "metadata_xml": sample_idp_metadata(),
        "default_role": "viewer",
    }))
    .send()
    .await?;
    assert_eq!(r.status().as_u16(), 403);

    Ok(())
}
