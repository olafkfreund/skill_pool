//! OIDC service-provider endpoints.
//!
//! Per-tenant config lives in `tenant_sso`. The dance:
//!
//! ```text
//! web/login -> GET /v1/auth/oidc/{tenant}/start?return_to=...
//!              -> 302 to IdP authorize URL (with state + PKCE)
//! IdP -> GET /v1/auth/oidc/{tenant}/callback?code=...&state=...
//!              -> exchange + validate ID token + upsert user + mint session
//!              -> 303 to {return_to}?token=...&tenant=...
//! web/oidc-return -> set cookies, redirect to /
//! ```
//!
//! Session tokens go in `user_sessions`; the auth extractor checks them
//! alongside the API-token table.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use base64::Engine;
use chrono::{Duration, Utc};
use hmac::{Hmac, KeyInit, Mac};
use openidconnect::core::{CoreClient, CoreProviderMetadata, CoreResponseType};
use openidconnect::{
    AuthenticationFlow, AuthorizationCode, ClientId, ClientSecret, CsrfToken, EndpointMaybeSet,
    EndpointNotSet, EndpointSet, IssuerUrl, Nonce, PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl, Scope,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::env;
use uuid::Uuid;

use crate::auth::hash_token;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

type HmacSha256 = Hmac<Sha256>;

/// The concrete `CoreClient` type returned by `CoreClient::from_provider_metadata`.
///
/// `from_provider_metadata` always sets the auth URL (`EndpointSet`). The
/// token URL and user-info URL are conditional (`EndpointMaybeSet`) because
/// they may or may not appear in the provider's OIDC discovery document.
/// Device-auth, introspection, and revocation URLs are not part of OIDC Core
/// discovery metadata and are left `EndpointNotSet` by `from_provider_metadata`.
type DiscoveredCoreClient = CoreClient<
    EndpointSet,      // HasAuthUrl           — always present in OIDC Core discovery
    EndpointNotSet,   // HasDeviceAuthUrl     — not part of OIDC Core discovery
    EndpointNotSet,   // HasIntrospectionUrl  — not part of OIDC Core discovery
    EndpointNotSet,   // HasRevocationUrl     — not part of OIDC Core discovery
    EndpointMaybeSet, // HasTokenUrl          — present in all conformant providers
    EndpointMaybeSet, // HasUserInfoUrl       — present in all conformant providers
>;

const STATE_COOKIE: &str = "sp_oidc_state";
const STATE_TTL: i64 = 600; // 10 minutes
const SESSION_TTL_DAYS: i64 = 14;

// --- Discovery (web asks: is SSO enabled here?) ---------------------------

#[derive(Serialize)]
pub struct OidcDiscovery {
    enabled: bool,
}

pub async fn discover(
    State(state): State<AppState>,
    tenant: TenantCtx,
) -> AppResult<Json<OidcDiscovery>> {
    let row = sqlx::query!(
        "SELECT issuer_url FROM tenant_sso WHERE tenant_id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;
    Ok(Json(OidcDiscovery {
        enabled: row.is_some(),
    }))
}

// --- Start the dance ------------------------------------------------------

#[derive(Deserialize)]
pub struct StartQuery {
    return_to: String,
}

pub async fn start(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(_slug): Path<String>,
    Query(q): Query<StartQuery>,
) -> AppResult<Response> {
    let sso = load_sso(&state, tenant.tenant_id).await?;
    // The http client is not needed here (no token exchange), but we keep the
    // tuple destructure consistent with callback's usage.
    let (client, _http) = build_client(&sso, &tenant.tenant_slug).await?;

    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_state, nonce) = client
        .authorize_url(
            AuthenticationFlow::<CoreResponseType>::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("email".into()))
        .add_scope(Scope::new("profile".into()))
        .set_pkce_challenge(pkce_challenge)
        .url();

    let blob = OidcStateBlob {
        csrf: csrf_state.secret().clone(),
        nonce: nonce.secret().clone(),
        pkce: pkce_verifier.secret().clone(),
        return_to: q.return_to,
        expires_at: Utc::now().timestamp() + STATE_TTL,
    };
    let cookie = format!(
        "{STATE_COOKIE}={}; Path=/v1/auth/oidc; HttpOnly; SameSite=Lax; Max-Age={STATE_TTL}",
        sign_blob(&blob)?
    );

    let mut resp = Redirect::temporary(auth_url.as_str()).into_response();
    resp.headers_mut()
        .insert("set-cookie", cookie.parse().expect("cookie value"));
    Ok(resp)
}

// --- Callback -------------------------------------------------------------

#[derive(Deserialize)]
pub struct CallbackQuery {
    code: String,
    state: String,
}

pub async fn callback(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(_slug): Path<String>,
    headers: axum::http::HeaderMap,
    Query(q): Query<CallbackQuery>,
) -> AppResult<Response> {
    let blob = take_state_cookie(&headers)?;

    if !constant_time_eq(blob.csrf.as_bytes(), q.state.as_bytes()) {
        return Err(AppError::BadRequest("state mismatch".into()));
    }
    if Utc::now().timestamp() > blob.expires_at {
        return Err(AppError::BadRequest("oidc state expired".into()));
    }

    let sso = load_sso(&state, tenant.tenant_id).await?;
    let (client, http) = build_client(&sso, &tenant.tenant_slug).await?;

    // In openidconnect 4.x, `exchange_code` returns `Result<_, ConfigurationError>`
    // when the client was built from provider metadata (`EndpointMaybeSet`), because
    // the token URL may not be present in the discovery document.
    let token_response = client
        .exchange_code(AuthorizationCode::new(q.code))
        .map_err(|e| AppError::BadRequest(format!("oidc token endpoint not configured: {e}")))?
        .set_pkce_verifier(PkceCodeVerifier::new(blob.pkce))
        .request_async(&http)
        .await
        .map_err(|e| AppError::BadRequest(format!("oidc code exchange failed: {e}")))?;

    let id_token = token_response
        .extra_fields()
        .id_token()
        .ok_or_else(|| AppError::BadRequest("oidc response missing id_token".into()))?;
    let claims = id_token
        .claims(&client.id_token_verifier(), &Nonce::new(blob.nonce))
        .map_err(|e| AppError::BadRequest(format!("oidc id_token rejected: {e}")))?;

    let email = claims
        .email()
        .map(|e| e.to_string())
        .ok_or_else(|| AppError::BadRequest("oidc id_token has no email".into()))?;
    let sub = claims.subject().to_string();
    let display_name = claims
        .name()
        .and_then(|n| n.get(None).map(|s| s.to_string()))
        .or_else(|| claims.preferred_username().map(|s| s.to_string()));

    // Pull groups from the verified id_token JWT payload. openidconnect's
    // typed claims don't expose `groups` natively (it's a non-standard claim
    // commonly emitted by Okta/Authentik/Azure); we already verified the
    // token's signature above, so it's safe to re-parse the payload.
    let groups = extract_groups_from_jwt(&id_token.to_string());

    let user_id = upsert_user(&state, &email, &sub, display_name.as_deref()).await?;
    ensure_membership(&state, tenant.tenant_id, user_id, &sso.default_role).await?;
    let _ =
        crate::auth::apply_role_from_groups(state.db(), tenant.tenant_id, user_id, &groups).await?;

    let session_token = mint_session(&state, tenant.tenant_id, user_id).await?;

    let return_url = format!(
        "{}?token={}&tenant={}",
        blob.return_to,
        urlencoding::encode(&session_token),
        urlencoding::encode(&tenant.tenant_slug),
    );

    let mut resp = Redirect::to(&return_url).into_response();
    // Clear the state cookie.
    resp.headers_mut().insert(
        "set-cookie",
        format!("{STATE_COOKIE}=; Path=/v1/auth/oidc; Max-Age=0")
            .parse()
            .expect("cookie value"),
    );
    Ok(resp)
}

// --- Helpers --------------------------------------------------------------

struct SsoConfig {
    issuer_url: String,
    client_id: String,
    client_secret: String,
    default_role: String,
}

async fn load_sso(state: &AppState, tenant_id: Uuid) -> AppResult<SsoConfig> {
    let row = sqlx::query!(
        "SELECT issuer_url, client_id, client_secret, default_role \
         FROM tenant_sso WHERE tenant_id = $1",
        tenant_id,
    )
    .fetch_optional(state.db())
    .await?
    .ok_or_else(|| AppError::BadRequest("OIDC not configured for this tenant".into()))?;
    Ok(SsoConfig {
        issuer_url: row.issuer_url,
        client_id: row.client_id,
        client_secret: row.client_secret,
        default_role: row.default_role,
    })
}

/// Build a `reqwest::Client` configured for OIDC use.
///
/// Redirects are disabled per the openidconnect 4.x recommendation to prevent
/// SSRF: <https://docs.rs/openidconnect/4.0.1/openidconnect/#openid-connect-discovery>
fn make_http_client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| AppError::Anyhow(anyhow::anyhow!("failed to build HTTP client: {e}")))
}

/// Run OIDC Discovery and construct a `CoreClient` ready for the auth-code flow.
///
/// Returns both the discovered client and the `reqwest::Client` so the same
/// HTTP connection pool can be reused for the subsequent token exchange without
/// creating a second client.
async fn build_client(
    sso: &SsoConfig,
    tenant_slug: &str,
) -> AppResult<(DiscoveredCoreClient, reqwest::Client)> {
    let http = make_http_client()?;
    let metadata = CoreProviderMetadata::discover_async(
        IssuerUrl::new(sso.issuer_url.clone())
            .map_err(|e| AppError::BadRequest(format!("bad issuer URL: {e}")))?,
        &http,
    )
    .await
    .map_err(|e| AppError::BadRequest(format!("oidc discovery failed: {e}")))?;

    let origin = env::var("SKILL_POOL_PUBLIC_ORIGIN")
        .unwrap_or_else(|_| "http://localhost:8080".to_string());
    let redirect = format!(
        "{}/v1/auth/oidc/{}/callback",
        origin.trim_end_matches('/'),
        tenant_slug
    );

    let client = CoreClient::from_provider_metadata(
        metadata,
        ClientId::new(sso.client_id.clone()),
        Some(ClientSecret::new(sso.client_secret.clone())),
    )
    .set_redirect_uri(
        RedirectUrl::new(redirect).map_err(|e| AppError::BadRequest(e.to_string()))?,
    );

    Ok((client, http))
}

fn extract_groups_from_jwt(jwt: &str) -> Vec<String> {
    let parts: Vec<&str> = jwt.split('.').collect();
    if parts.len() != 3 {
        return vec![];
    }
    let Ok(payload_bytes) =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(parts[1].as_bytes())
    else {
        return vec![];
    };
    let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&payload_bytes) else {
        return vec![];
    };
    extract_groups_from_json(&payload)
}

fn extract_groups_from_json(claims: &serde_json::Value) -> Vec<String> {
    // Common claim names. `groups` covers Okta/Authentik/Keycloak; `roles`
    // is Azure AD when an app role mapping is configured; `memberOf` shows
    // up in some custom mappers (esp. legacy LDAP bridges).
    for key in ["groups", "roles", "memberOf"] {
        if let Some(arr) = claims.get(key).and_then(|v| v.as_array()) {
            return arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }
    vec![]
}

async fn upsert_user(
    state: &AppState,
    email: &str,
    external_idp_id: &str,
    display_name: Option<&str>,
) -> AppResult<Uuid> {
    let row = sqlx::query!(
        "INSERT INTO users (email, external_idp_id, display_name) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (email) DO UPDATE SET \
           external_idp_id = EXCLUDED.external_idp_id, \
           display_name = COALESCE(EXCLUDED.display_name, users.display_name) \
         RETURNING id",
        email,
        external_idp_id,
        display_name,
    )
    .fetch_one(state.db())
    .await?;
    Ok(row.id)
}

async fn ensure_membership(
    state: &AppState,
    tenant_id: Uuid,
    user_id: Uuid,
    default_role: &str,
) -> AppResult<()> {
    sqlx::query!(
        "INSERT INTO tenant_users (tenant_id, user_id, role) \
         VALUES ($1, $2, $3) \
         ON CONFLICT (tenant_id, user_id) DO NOTHING",
        tenant_id,
        user_id,
        default_role,
    )
    .execute(state.db())
    .await?;
    Ok(())
}

async fn mint_session(state: &AppState, tenant_id: Uuid, user_id: Uuid) -> AppResult<String> {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let raw = format!("sps_{}", hex::encode(bytes));
    let hashed = hash_token(&raw);
    let expires_at = Utc::now() + Duration::days(SESSION_TTL_DAYS);

    sqlx::query!(
        "INSERT INTO user_sessions (tenant_id, user_id, hashed_token, expires_at) \
         VALUES ($1, $2, $3, $4)",
        tenant_id,
        user_id,
        &hashed,
        expires_at,
    )
    .execute(state.db())
    .await?;

    Ok(raw)
}

// --- State cookie (HMAC-signed JSON blob) --------------------------------

#[derive(Serialize, Deserialize)]
struct OidcStateBlob {
    csrf: String,
    nonce: String,
    pkce: String,
    return_to: String,
    expires_at: i64,
}

fn signing_key() -> Vec<u8> {
    // Pulled from env at every sign/verify; if missing, fall back to a fixed
    // dev value with a loud warning so dev still works without ceremony.
    match env::var("SKILL_POOL_COOKIE_SECRET") {
        Ok(s) if !s.is_empty() => s.into_bytes(),
        _ => {
            tracing::warn!(
                "SKILL_POOL_COOKIE_SECRET not set; using insecure dev fallback. Set it in production."
            );
            b"dev-only-skill-pool-cookie-secret-rotate-me".to_vec()
        }
    }
}

fn sign_blob(blob: &OidcStateBlob) -> AppResult<String> {
    let json = serde_json::to_vec(blob).map_err(|e| AppError::Anyhow(e.into()))?;
    let mut mac = HmacSha256::new_from_slice(&signing_key())
        .map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
    mac.update(&json);
    let tag = mac.finalize().into_bytes();
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    Ok(format!("{}.{}", b64.encode(&json), b64.encode(&tag[..])))
}

fn verify_blob(s: &str) -> AppResult<OidcStateBlob> {
    let (payload, tag) = s
        .split_once('.')
        .ok_or_else(|| AppError::BadRequest("malformed state cookie".into()))?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD;
    let json = b64
        .decode(payload)
        .map_err(|_| AppError::BadRequest("state cookie payload".into()))?;
    let tag_bytes = b64
        .decode(tag)
        .map_err(|_| AppError::BadRequest("state cookie tag".into()))?;

    let mut mac = HmacSha256::new_from_slice(&signing_key())
        .map_err(|e| AppError::Anyhow(anyhow::anyhow!(e)))?;
    mac.update(&json);
    mac.verify_slice(&tag_bytes)
        .map_err(|_| AppError::BadRequest("state cookie HMAC mismatch".into()))?;

    serde_json::from_slice(&json).map_err(|e| AppError::BadRequest(format!("state json: {e}")))
}

fn take_state_cookie(headers: &axum::http::HeaderMap) -> AppResult<OidcStateBlob> {
    let raw = headers
        .get("cookie")
        .and_then(|h| h.to_str().ok())
        .ok_or_else(|| AppError::BadRequest("missing state cookie".into()))?;
    for part in raw.split(';') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix(&format!("{STATE_COOKIE}=")) {
            return verify_blob(v);
        }
    }
    Err(AppError::BadRequest("missing state cookie".into()))
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut acc: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        acc |= x ^ y;
    }
    acc == 0
}

// --- Status endpoint for the web's /oidc-return page ---------------------

#[derive(Serialize)]
pub struct WhoAmI {
    user_id: Uuid,
    email: String,
    role: String,
    tenant: String,
}

pub async fn whoami(
    State(state): State<AppState>,
    caller: crate::auth::AuthedCaller,
) -> AppResult<Json<WhoAmI>> {
    // The session-token auth path populates user_id; the API-token path doesn't.
    let user_id = caller.user_id.ok_or_else(|| AppError::Unauthorized)?;

    let row = sqlx::query!(
        "SELECT u.email, tu.role \
         FROM users u JOIN tenant_users tu ON tu.user_id = u.id \
         WHERE u.id = $1 AND tu.tenant_id = $2",
        user_id,
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?
    .ok_or(AppError::NotFound)?;
    let (email, role) = (row.email, row.role);

    Ok(Json(WhoAmI {
        user_id,
        email,
        role,
        tenant: caller.tenant.tenant_slug,
    }))
}

// --- Logout (revokes the session) ----------------------------------------

pub async fn logout(
    State(state): State<AppState>,
    caller: crate::auth::AuthedCaller,
) -> AppResult<StatusCode> {
    if let Some(session_id) = caller.session_id {
        sqlx::query!(
            "UPDATE user_sessions SET revoked_at = now() WHERE id = $1",
            session_id,
        )
        .execute(state.db())
        .await?;
    }
    Ok(StatusCode::NO_CONTENT)
}
