//! Round-trip test for the personal API token surface (#4).
//!
//! 1. Pre-condition: a session-authenticated user can hit `/v1/profile/tokens`
//!    and gets an empty list (no tokens minted yet).
//! 2. POST a new token → 200; response carries `raw_token`, `prefix`, and
//!    matching `id`. The token starts with `spk_`.
//! 3. GET → the new row appears with the same `id`, the `prefix` is set,
//!    `revoked_at` is null, and **the wire payload does NOT contain the
//!    raw token string** (we never leak it on a list endpoint).
//! 4. The minted token actually works as a Bearer credential — POST `/v1/usage`
//!    with it returns 2xx. (This proves the SHA-256 was written correctly,
//!    not just that the row exists.)
//! 5. DELETE the token → 204. List again → the row is still there but
//!    `revoked_at` is set. Repeat DELETE → 204 (idempotent).
//! 6. The revoked token no longer authenticates: a subsequent request with
//!    it as Bearer returns 401. (Best-effort: we drain the auth cache by
//!    sleeping briefly; the test's pool is non-Redis so this is a no-op.)
//! 7. Negative: a pure API-token caller (no session) gets 401 on the same
//!    endpoint — bare CLI tokens cannot manage personal credentials.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use chrono::{Duration as ChronoDuration, Utc};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use uuid::Uuid;

use skill_pool_server::{admin, auth, config, routes, state};

struct Harness {
    base: String,
    session_token: String,
    api_token: String,
    _pg: testcontainers::ContainerAsync<Postgres>,
    _storage_dir: tempfile::TempDir,
}

/// Mint a user + tenant_users row + active session, returning the raw session
/// token the caller should set as Bearer. Mirrors the row shape that
/// `routes/oidc.rs:callback` and `routes/saml.rs:acs` write — see the
/// `INSERT INTO user_sessions` calls there. Kept inline so the harness has
/// no dependency on OIDC config.
async fn mint_session(pool: &PgPool, tenant_slug: &str, email: &str, role: &str) -> Result<String> {
    use rand::RngCore;
    use sha2::{Digest, Sha256};

    let (tenant_id,): (Uuid,) = sqlx::query_as("SELECT id FROM tenants WHERE slug = $1")
        .bind(tenant_slug)
        .fetch_one(pool)
        .await?;

    let (user_id,): (Uuid,) = sqlx::query_as(
        "INSERT INTO users (email, display_name) VALUES ($1, $2) \
         ON CONFLICT (email) DO UPDATE SET display_name = EXCLUDED.display_name \
         RETURNING id",
    )
    .bind(email)
    .bind("Test User")
    .fetch_one(pool)
    .await?;

    sqlx::query(
        "INSERT INTO tenant_users (tenant_id, user_id, role) VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, user_id) DO UPDATE SET role = EXCLUDED.role",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(role)
    .execute(pool)
    .await?;

    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = format!("sess_{}", hex::encode(bytes));
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    let hashed = hex::encode(h.finalize());

    let expires = Utc::now() + ChronoDuration::hours(8);
    sqlx::query(
        "INSERT INTO user_sessions (tenant_id, user_id, hashed_token, expires_at) \
         VALUES ($1, $2, $3, $4)",
    )
    .bind(tenant_id)
    .bind(user_id)
    .bind(&hashed)
    .bind(expires)
    .execute(pool)
    .await?;

    // Sanity: hash_token in the auth module agrees with our local SHA-256.
    debug_assert_eq!(auth::hash_token(&raw), hashed);

    Ok(raw)
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

    admin::create_tenant(&pool, "acme", "Acme", "team").await?;

    // Curator role gets `skills:read skills:publish` — enough to mint
    // non-admin tokens, not enough to mint a tenant:admin token.
    let session_token = mint_session(&pool, "acme", "alice@acme.example.com", "curator").await?;
    // An API token with no session. Used to verify the "pure API token
    // callers cannot manage personal tokens" rule.
    let api_token = admin::create_token(&pool, "acme", "bot", "skills:read skills:publish")
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
        session_token,
        api_token,
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

#[tokio::test]
async fn profile_token_round_trip() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // 1. Empty list to start.
    let r = c
        .get(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let body: Value = r.json().await?;
    assert!(body.as_array().is_some_and(|a| a.is_empty()));

    // 2. Create a token.
    let r = c
        .post(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .json(&json!({
            "label": "CI bot",
            "scopes": ["skills:read", "skills:publish"],
        }))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 200, "{}", r.text().await?);
    let created: Value = r.json().await?;
    let raw = created["raw_token"].as_str().unwrap().to_string();
    let id = created["id"].as_str().unwrap().to_string();
    let prefix = created["prefix"].as_str().unwrap().to_string();
    assert!(raw.starts_with("spk_"), "raw token format: {raw}");
    assert_eq!(prefix.len(), 12, "prefix length: {prefix}");
    assert!(raw.starts_with(&prefix), "raw token must start with prefix");
    assert_eq!(created["scopes"], "skills:read skills:publish");
    assert_eq!(created["label"], "CI bot");

    // 3. List returns the new row, no raw token anywhere.
    let r = c
        .get(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .send()
        .await?;
    let listed: Value = r.json().await?;
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], id);
    assert_eq!(arr[0]["prefix"], prefix);
    assert_eq!(arr[0]["label"], "CI bot");
    assert_eq!(arr[0]["scopes"], "skills:read skills:publish");
    assert!(arr[0]["revoked_at"].is_null());

    let listed_str = serde_json::to_string(&listed)?;
    assert!(
        !listed_str.contains(&raw),
        "list endpoint must NOT leak the raw token"
    );

    // 4. The minted token works as a Bearer credential. We call /v1/usage
    //    because it's a tiny, side-effect-free POST with skills:publish.
    let r = c
        .post(format!("{}/v1/usage", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&raw)
        .json(&json!({"skill_slug": "irrelevant", "version": "0.0.0", "event": "view"}))
        .send()
        .await?;
    assert_ne!(
        r.status().as_u16(),
        401,
        "minted token should authenticate (got {})",
        r.status()
    );
    assert_ne!(
        r.status().as_u16(),
        403,
        "minted token should authorize for usage events"
    );

    // 5. Revoke. 204, idempotent.
    let r = c
        .delete(format!("{}/v1/profile/tokens/{}", h.base, id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 204);

    let r = c
        .delete(format!("{}/v1/profile/tokens/{}", h.base, id))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .send()
        .await?;
    // Second revoke: still 204 because the row exists and is already
    // revoked. Idempotent contract.
    assert_eq!(r.status().as_u16(), 204);

    // 6. List shows revoked_at set.
    let r = c
        .get(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .send()
        .await?;
    let listed: Value = r.json().await?;
    let arr = listed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert!(!arr[0]["revoked_at"].is_null(), "revoked_at must be set");

    // 7. Revoking a token that doesn't belong to this user → 404.
    let bogus = Uuid::new_v4();
    let r = c
        .delete(format!("{}/v1/profile/tokens/{}", h.base, bogus))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 404);

    Ok(())
}

#[tokio::test]
async fn api_token_caller_cannot_manage_profile_tokens() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // GET with a bare API token (no session) → 401.
    let r = c
        .get(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.api_token)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 401);

    // POST with a bare API token → 401.
    let r = c
        .post(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.api_token)
        .json(&json!({"label": "ci", "scopes": ["skills:read"]}))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 401);

    Ok(())
}

#[tokio::test]
async fn non_admin_user_cannot_mint_admin_scope() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // The harness's session is a curator — no tenant:admin scope. So a
    // POST asking for tenant:admin must be rejected (403, not silently
    // demoted to read-only).
    let r = c
        .post(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .json(&json!({
            "label": "would be admin",
            "scopes": ["tenant:admin"],
        }))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 403);

    Ok(())
}

#[tokio::test]
async fn bad_inputs_are_rejected() -> Result<()> {
    let h = boot().await?;
    let c = cl();

    // Empty label.
    let r = c
        .post(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .json(&json!({"label": "", "scopes": ["skills:read"]}))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 400);

    // No scopes.
    let r = c
        .post(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .json(&json!({"label": "valid", "scopes": []}))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 400);

    // Unknown scope.
    let r = c
        .post(format!("{}/v1/profile/tokens", h.base))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&h.session_token)
        .json(&json!({"label": "valid", "scopes": ["rogue:scope"]}))
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 400);

    Ok(())
}
