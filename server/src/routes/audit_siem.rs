//! Tenant SIEM export settings (Phase 5).
//!
//! - `GET /v1/tenant/audit-siem` — admin only; current URL + whether a
//!   token is configured. The actual token is never returned over the
//!   wire — the response surfaces a `token_configured` bool instead so
//!   the UI can show "token set" without leaking it.
//! - `PUT /v1/tenant/audit-siem` — admin only; set/clear the URL and
//!   token. `None` keys leave the existing value alone; empty strings
//!   clear; non-empty strings replace.
//!
//! This is the control plane for the fan-out implemented in `audit.rs`.
//! It's a separate endpoint from `/v1/tenant/notifications` because the
//! audit firehose targets SIEM platforms (Splunk HEC, Datadog Logs)
//! with a different payload shape (one row = one POST, full audit
//! schema), whereas the curator webhook is a single user-readable
//! envelope on draft events.

use axum::extract::State;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Serialize, Deserialize)]
pub struct AuditSiemConfig {
    /// SIEM HTTP receiver URL. `None` = no SIEM export configured.
    /// Token is never returned: `token_configured` mirrors the
    /// boolean shape used by `signing_enabled` in the notification
    /// surface — same UX, same redaction rule.
    pub url: Option<String>,
    pub token_configured: bool,
}

pub async fn get_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<AuditSiemConfig>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let row = sqlx::query!(
        "SELECT tenant_audit_siem_url, tenant_audit_siem_token \
         FROM tenants WHERE id = $1",
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;
    let (url, token) = row
        .map(|r| (r.tenant_audit_siem_url, r.tenant_audit_siem_token))
        .unwrap_or((None, None));
    Ok(Json(AuditSiemConfig {
        url,
        token_configured: token.is_some(),
    }))
}

#[derive(Deserialize)]
pub struct PutBody {
    /// `None` leaves the existing URL untouched; `Some("")` clears;
    /// `Some("…")` replaces. Same semantics as the notifications PUT.
    #[serde(default)]
    pub url: Option<String>,
    /// `None` leaves the existing token untouched; `Some("")` clears;
    /// `Some("…")` replaces. Sent to the SIEM as `Authorization: Bearer`.
    #[serde(default)]
    pub token: Option<String>,
}

pub async fn put_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<PutBody>,
) -> AppResult<Json<AuditSiemConfig>> {
    require_scope(&caller.scope, "tenant:admin")?;

    let normalize = |o: Option<String>| -> Option<Option<String>> {
        o.map(|s| {
            let t = s.trim().to_string();
            if t.is_empty() {
                None
            } else {
                Some(t)
            }
        })
    };
    let url = normalize(body.url);
    if let Some(Some(u)) = &url {
        if !is_acceptable_url(u) {
            return Err(AppError::BadRequest("url must be an http(s) URL".into()));
        }
    }
    let token = normalize(body.token);

    // JUSTIFIED runtime-checked: `$N::int = 0` flag parameters require an
    // explicit PostgreSQL cast that the `query!` macro cannot verify at
    // compile time for integer flag arguments paired with nullable text.
    // The CASE … ELSE pattern is the canonical partial-update idiom for
    // a fixed multi-column UPDATE where some columns are left unchanged.
    sqlx::query(
        "UPDATE tenants SET \
            tenant_audit_siem_url   = CASE WHEN $2::int = 0 THEN tenant_audit_siem_url   ELSE $3 END, \
            tenant_audit_siem_token = CASE WHEN $4::int = 0 THEN tenant_audit_siem_token ELSE $5 END, \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(if url.is_some() { 1_i32 } else { 0 })
    .bind(url.clone().unwrap_or(None))
    .bind(if token.is_some() { 1_i32 } else { 0 })
    .bind(token.clone().unwrap_or(None))
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.audit_siem.update",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({
                "url_changed": url.is_some(),
                "token_changed": token.is_some(),
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    get_config(State(state), caller).await
}

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn is_acceptable_url(s: &str) -> bool {
    url::Url::parse(s)
        .map(|u| matches!(u.scheme(), "http" | "https"))
        .unwrap_or(false)
}
