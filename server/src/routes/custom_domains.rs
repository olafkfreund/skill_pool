//! Per-tenant custom-domain admin flow (Phase 5 / Enterprise).
//!
//! Lets a tenant admin pin their own hostname — `skills.acme.com` — at
//! the same backend that normally serves `acme.skill-pool.example.com`.
//! The reverse proxy (Caddy `on_demand_tls`, Traefik per-host HTTP-01)
//! handles cert issuance; this module's job is the **control plane**:
//!
//!   1. `POST   /v1/tenant/custom-domains`           — claim a hostname
//!   2. `GET    /v1/tenant/custom-domains`           — list this tenant's
//!   3. `POST   /v1/tenant/custom-domains/{id}/verify` — DNS-TXT check
//!   4. `DELETE /v1/tenant/custom-domains/{id}`       — withdraw a claim
//!   5. `GET    /v1/tenant/custom-domains/{host}/cert-ok`
//!      — **no auth**, called by Caddy `on_demand_tls.ask` so random
//!      hostnames can't trigger ACME issuance against our backend.
//!
//! Status flow: pending → verified → active. `verified` is set when the
//! tenant has proven DNS control (a TXT record we generated for them);
//! `active` is set by the operator after the reverse proxy has been
//! wired up and the cert issued. The `Activate` admin CLI option flips
//! the state manually for cases where DNS verification is impractical
//! (internal CAs, pre-issued certs from a customer's PKI).
//!
//! See `docs/enterprise/custom-domains.md` for the end-to-end recipe.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::normalize_host;

/// Wire shape for a row. `verification_record` is a convenience: the
/// pre-formatted string the tenant admin pastes into their DNS panel.
#[derive(Serialize)]
pub struct CustomDomain {
    pub id: Uuid,
    pub hostname: String,
    pub status: String,
    /// Pre-formatted "host TXT value" the tenant adds to their DNS
    /// zone. Only meaningful while status is `pending` / `failed`;
    /// kept on the response even after verify so the admin UI can
    /// show "you previously added: …".
    pub verification_record: String,
    pub last_checked_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub activated_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

/// Column list shared by every read in this module. Keep the order in
/// sync with `CustomDomainRow` below.
///
/// JUSTIFIED runtime-checked: `COLS` is used via `format!()` to compose
/// INSERT RETURNING, SELECT, and UPDATE RETURNING queries at runtime. The
/// `query!` macro requires a single string literal — const-fragment
/// concatenation is not supported. All queries include `tenant_id = $N`
/// to enforce tenant scope.
const COLS: &str =
    "id, hostname, status, verification_token, last_checked_at, last_error, activated_at, created_at";

type CustomDomainRow = (
    Uuid,                     // id
    String,                   // hostname
    String,                   // status
    String,                   // verification_token
    Option<DateTime<Utc>>,    // last_checked_at
    Option<String>,           // last_error
    Option<DateTime<Utc>>,    // activated_at
    DateTime<Utc>,            // created_at
);

fn row_to_wire(row: CustomDomainRow) -> CustomDomain {
    let (id, hostname, status, token, last_checked_at, last_error, activated_at, created_at) = row;
    CustomDomain {
        verification_record: format_verification_record(&hostname, &token),
        id,
        hostname,
        status,
        last_checked_at,
        last_error,
        activated_at,
        created_at,
    }
}

/// The exact line a tenant pastes into their DNS panel.
pub(crate) fn format_verification_record(hostname: &str, token: &str) -> String {
    format!("_skill-pool-verify.{hostname} TXT {token}")
}

// ---------------------------------------------------------------------------
// POST /v1/tenant/custom-domains
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateBody {
    pub hostname: String,
}

pub async fn create(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<CreateBody>,
) -> AppResult<(StatusCode, Json<CustomDomain>)> {
    require_admin(&caller)?;
    let hostname = validate_hostname(&body.hostname)?;
    let token = generate_verification_token();

    let row: CustomDomainRow = sqlx::query_as(&format!(
        "INSERT INTO tenant_custom_domains (tenant_id, hostname, verification_token) \
         VALUES ($1, $2, $3) RETURNING {COLS}"
    ))
    .bind(caller.tenant.tenant_id)
    .bind(&hostname)
    .bind(&token)
    .fetch_one(state.db())
    .await
    .map_err(|e| {
        // Unique violation on hostname → 409-ish; the admin asked for a
        // host another tenant already claimed (or this tenant re-added).
        if let Some(db_err) = e.as_database_error() {
            if db_err.code().as_deref() == Some("23505") {
                return AppError::BadRequest(format!(
                    "hostname `{hostname}` is already registered"
                ));
            }
        }
        AppError::from(e)
    })?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.custom_domain.create",
            target_kind: "custom_domain",
            target_id: Some(&row.0.to_string()),
            metadata: serde_json::json!({ "hostname": hostname }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok((StatusCode::CREATED, Json(row_to_wire(row))))
}

// ---------------------------------------------------------------------------
// GET /v1/tenant/custom-domains
// ---------------------------------------------------------------------------

pub async fn list(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<Vec<CustomDomain>>> {
    require_admin(&caller)?;
    let sql = format!(
        "SELECT {COLS} FROM tenant_custom_domains WHERE tenant_id = $1 ORDER BY created_at DESC"
    );
    let rows: Vec<CustomDomainRow> = sqlx::query_as(&sql)
        .bind(caller.tenant.tenant_id)
        .fetch_all(state.db())
        .await?;
    Ok(Json(rows.into_iter().map(row_to_wire).collect()))
}

// ---------------------------------------------------------------------------
// POST /v1/tenant/custom-domains/{id}/verify
// ---------------------------------------------------------------------------

pub async fn verify(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<Json<CustomDomain>> {
    require_admin(&caller)?;

    // Reload the row to grab the hostname + token we need for the DNS
    // lookup. `FOR UPDATE` so two concurrent verify calls don't both
    // race to write `last_checked_at`.
    let mut tx = state.db().begin().await?;
    let row: Option<CustomDomainRow> = sqlx::query_as(&format!(
        "SELECT {COLS} FROM tenant_custom_domains \
         WHERE id = $1 AND tenant_id = $2 FOR UPDATE"
    ))
    .bind(id)
    .bind(caller.tenant.tenant_id)
    .fetch_optional(&mut *tx)
    .await?;
    let row = row.ok_or(AppError::NotFound)?;
    let hostname = row.1.clone();
    let token = row.3.clone();

    let outcome = lookup_verification_txt(&hostname, &token).await;

    let updated: CustomDomainRow = match outcome {
        Ok(()) => sqlx::query_as(&format!(
            "UPDATE tenant_custom_domains \
                SET status = 'verified', \
                    last_checked_at = now(), \
                    last_error = NULL, \
                    activated_at = COALESCE(activated_at, now()) \
              WHERE id = $1 AND tenant_id = $2 \
              RETURNING {COLS}"
        ))
        .bind(id)
        .bind(caller.tenant.tenant_id)
        .fetch_one(&mut *tx)
        .await?,
        Err(err) => sqlx::query_as(&format!(
            "UPDATE tenant_custom_domains \
                SET status = 'failed', \
                    last_checked_at = now(), \
                    last_error = $3 \
              WHERE id = $1 AND tenant_id = $2 \
              RETURNING {COLS}"
        ))
        .bind(id)
        .bind(caller.tenant.tenant_id)
        .bind(err.clone())
        .fetch_one(&mut *tx)
        .await?,
    };

    tx.commit().await?;

    // Whether we flipped to verified OR failed, the cache contents may
    // have changed (verified means we need to add to cache; previously
    // verified failing means we need to drop it). Refresh once.
    if let Err(e) = state.refresh_custom_domains().await {
        tracing::warn!(error = ?e, "refresh custom-domain cache after verify failed");
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.custom_domain.verify",
            target_kind: "custom_domain",
            target_id: Some(&id.to_string()),
            metadata: serde_json::json!({
                "hostname": hostname,
                "status": updated.2,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(Json(row_to_wire(updated)))
}

// ---------------------------------------------------------------------------
// DELETE /v1/tenant/custom-domains/{id}
// ---------------------------------------------------------------------------

pub async fn remove(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    require_admin(&caller)?;

    let removed = sqlx::query!(
        "DELETE FROM tenant_custom_domains \
         WHERE id = $1 AND tenant_id = $2 RETURNING hostname",
        id,
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;
    let hostname = removed.ok_or(AppError::NotFound)?.hostname;

    if let Err(e) = state.refresh_custom_domains().await {
        tracing::warn!(error = ?e, "refresh custom-domain cache after delete failed");
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.custom_domain.delete",
            target_kind: "custom_domain",
            target_id: Some(&id.to_string()),
            metadata: serde_json::json!({ "hostname": hostname }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// GET /v1/tenant/custom-domains/{host}/cert-ok  (no auth, no tenant ctx)
// ---------------------------------------------------------------------------

/// Called by the reverse proxy's `on_demand_tls.ask` hook (Caddy) — or
/// the equivalent Traefik allow-list — BEFORE issuing a cert for a
/// hostname. Returns 200 when the hostname is in `verified` or `active`
/// status, 404 otherwise. No auth, no tenant context: by design, the
/// proxy talks to us pre-TLS, so the only thing we know is the SNI / Host
/// it's about to vend a cert for.
///
/// Defends against random-hostname cert-flood attacks: an attacker
/// pointing `evil.example.com` at our backend won't trigger an ACME
/// request because this endpoint will 404 and the proxy will refuse to
/// continue.
pub async fn cert_ok(
    State(state): State<AppState>,
    Path(host): Path<String>,
) -> StatusCode {
    let host = normalize_host(&host);
    if state.custom_domain_tenant(&host).await.is_some() {
        StatusCode::OK
    } else {
        StatusCode::NOT_FOUND
    }
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn require_admin(caller: &AuthedCaller) -> AppResult<()> {
    if caller
        .scope
        .split_whitespace()
        .any(|s| s == "tenant:admin" || s == "*")
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

/// Lower-cases, strips trailing dot, and verifies the hostname looks
/// like a public DNS name. Deliberately conservative — we'd rather a
/// tenant retry with a sanitised value than have a malformed Host sneak
/// into the unique index.
fn validate_hostname(raw: &str) -> AppResult<String> {
    let h = raw.trim().trim_end_matches('.').to_lowercase();
    if h.is_empty() {
        return Err(AppError::BadRequest("hostname is required".into()));
    }
    if h.len() > 253 {
        return Err(AppError::BadRequest("hostname exceeds 253 chars".into()));
    }
    if !h.contains('.') {
        return Err(AppError::BadRequest(
            "hostname must be fully qualified (contain a `.`)".into(),
        ));
    }
    for label in h.split('.') {
        if label.is_empty() || label.len() > 63 {
            return Err(AppError::BadRequest(
                "hostname labels must be 1..63 characters".into(),
            ));
        }
        if !label
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        {
            return Err(AppError::BadRequest(
                "hostname labels must be ASCII alphanumeric or `-`".into(),
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Err(AppError::BadRequest(
                "hostname labels must not begin or end with `-`".into(),
            ));
        }
    }
    Ok(h)
}

fn generate_verification_token() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

/// Perform a TXT lookup at `_skill-pool-verify.<hostname>`. Returns
/// `Ok(())` when at least one TXT record matches `expected` exactly;
/// otherwise returns a human-readable error string suitable for
/// `last_error`.
///
/// **Test override**: if `SKILL_POOL_DNS_VERIFY_OVERRIDE` is set, it is
/// parsed as a `hostname=token[,hostname=token,...]` allow-list. Used
/// by the integration test to avoid depending on real DNS.
async fn lookup_verification_txt(hostname: &str, expected: &str) -> Result<(), String> {
    if let Some(env) = test_override() {
        return match env.get(hostname) {
            Some(t) if t == expected => Ok(()),
            Some(_) => Err("test override: token mismatch".into()),
            None => Err(format!("test override: no TXT for {hostname}")),
        };
    }

    use hickory_resolver::config::{ResolverConfig, ResolverOpts};
    use hickory_resolver::TokioAsyncResolver;

    let lookup_name = format!("_skill-pool-verify.{hostname}");

    // Use the host's `/etc/resolv.conf` when available; fall back to a
    // sensible public default so a bare container without a configured
    // resolver still works.
    let resolver = match TokioAsyncResolver::tokio_from_system_conf() {
        Ok(r) => r,
        Err(_) => {
            TokioAsyncResolver::tokio(ResolverConfig::cloudflare(), ResolverOpts::default())
        }
    };

    let txt = match resolver.txt_lookup(&lookup_name).await {
        Ok(t) => t,
        Err(e) => return Err(format!("TXT lookup for {lookup_name} failed: {e}")),
    };

    for record in txt.iter() {
        for chunk in record.iter() {
            if let Ok(s) = std::str::from_utf8(chunk) {
                if s.trim() == expected {
                    return Ok(());
                }
            }
        }
    }
    Err(format!(
        "no TXT record at {lookup_name} matched the expected verification token"
    ))
}

/// Parse `SKILL_POOL_DNS_VERIFY_OVERRIDE=foo.com=abc,bar.com=def` into a
/// lookup map. Returns `None` when the env var is unset.
///
/// Production deploys never set this; it exists so the integration test
/// can drive the verify path without a real DNS zone.
fn test_override() -> Option<std::collections::HashMap<String, String>> {
    let raw = std::env::var("SKILL_POOL_DNS_VERIFY_OVERRIDE").ok()?;
    let mut out = std::collections::HashMap::new();
    for pair in raw.split(',') {
        let mut kv = pair.splitn(2, '=');
        if let (Some(k), Some(v)) = (kv.next(), kv.next()) {
            out.insert(k.trim().to_lowercase(), v.trim().to_string());
        }
    }
    Some(out)
}
