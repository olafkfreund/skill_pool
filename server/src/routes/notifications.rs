//! Tenant notification settings (Phase 5).
//!
//! - `GET  /v1/tenant/notifications` — admin only; current webhook config
//! - `PUT  /v1/tenant/notifications` — admin only; set/unset URL + secret
//! - `GET  /v1/tenant/notifications/pending-count` — read-only; count of
//!   pending drafts. Used by the web sidebar badge so curators see "3"
//!   without opening the inbox.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

#[derive(Serialize, Deserialize)]
pub struct NotificationsConfig {
    /// Webhook URL. `None` = no webhook configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub webhook_url: Option<String>,
    /// Whether a signing secret is configured. The actual secret is
    /// never returned over the wire — we surface a boolean so the UI
    /// can render "secret is set" without leaking it.
    pub signing_enabled: bool,
}

pub async fn get_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<NotificationsConfig>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT notifications_webhook_url, notifications_webhook_secret \
         FROM tenants WHERE id = $1",
    )
    .bind(caller.tenant.tenant_id)
    .fetch_optional(state.db())
    .await?;
    let (url, secret) = row.unwrap_or((None, None));
    Ok(Json(NotificationsConfig {
        webhook_url: url,
        signing_enabled: secret.is_some(),
    }))
}

#[derive(Deserialize)]
pub struct PutBody {
    /// `None` or empty string clears the webhook.
    #[serde(default)]
    pub webhook_url: Option<String>,
    /// `None` leaves the existing secret untouched; empty string clears it.
    /// A non-empty string replaces it.
    #[serde(default)]
    pub webhook_secret: Option<String>,
}

pub async fn put_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<PutBody>,
) -> AppResult<Json<NotificationsConfig>> {
    require_scope(&caller.scope, "tenant:admin")?;

    // Normalise empty string → NULL so the SQL stays clean.
    let url = body
        .webhook_url
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if let Some(u) = &url {
        if !is_acceptable_url(u) {
            return Err(AppError::BadRequest(
                "webhook_url must be an http(s) URL".into(),
            ));
        }
    }

    let secret_changed = body.webhook_secret.is_some();
    match body.webhook_secret {
        None => {
            // Leave secret untouched.
            sqlx::query(
                "UPDATE tenants SET notifications_webhook_url = $1, updated_at = now() \
                 WHERE id = $2",
            )
            .bind(url.as_ref())
            .bind(caller.tenant.tenant_id)
            .execute(state.db())
            .await?;
        }
        Some(s) => {
            // Empty string clears; non-empty replaces.
            let new_secret = if s.trim().is_empty() {
                None
            } else {
                Some(s.trim().to_string())
            };
            sqlx::query(
                "UPDATE tenants SET notifications_webhook_url = $1, \
                                    notifications_webhook_secret = $2, \
                                    updated_at = now() \
                 WHERE id = $3",
            )
            .bind(url.as_ref())
            .bind(new_secret.as_ref())
            .bind(caller.tenant.tenant_id)
            .execute(state.db())
            .await?;
        }
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.notifications.update",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({
                "webhook_url_set": url.is_some(),
                "secret_changed": secret_changed,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    // Reload + return the same shape as GET.
    get_config(State(state), caller).await
}

#[derive(Serialize)]
pub struct PendingCount {
    pub pending: i64,
}

pub async fn pending_count(
    State(state): State<AppState>,
    _caller: AuthedCaller,
    tenant: TenantCtx,
) -> AppResult<Json<PendingCount>> {
    let (n,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM skill_drafts WHERE tenant_id = $1 AND status = 'pending'",
    )
    .bind(tenant.tenant_id)
    .fetch_one(state.db())
    .await?;
    Ok(Json(PendingCount { pending: n }))
}

// --- helpers --------------------------------------------------------------

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

#[allow(dead_code)]
const _STATUS_OK: StatusCode = StatusCode::OK;
