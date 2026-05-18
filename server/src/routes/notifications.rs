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
    /// SMTP URL (`smtp://...` or `smtps://...`). Returned to admins so
    /// they can see what's configured; the password embedded in the
    /// userinfo segment is left in place — operators wanting to hide
    /// it should mint a token-scoped SMTP credential.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_url: Option<String>,
    /// `Name <addr@host>` or bare `addr@host`. Used as the From header.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_from: Option<String>,
    /// To address — a single mailbox or distribution-list address.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_to: Option<String>,
}

/// Row tuple returned by the combined config query (webhook + SMTP).
/// Aliased to keep clippy's `type_complexity` quiet.
type ConfigRow = (
    Option<String>, // webhook url
    Option<String>, // webhook secret
    Option<String>, // smtp url
    Option<String>, // smtp from
    Option<String>, // smtp to
);

pub async fn get_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<NotificationsConfig>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let row: Option<ConfigRow> = sqlx::query_as(
        "SELECT notifications_webhook_url, notifications_webhook_secret, \
                notification_smtp_url, notification_smtp_from, notification_smtp_to \
         FROM tenants WHERE id = $1",
    )
    .bind(caller.tenant.tenant_id)
    .fetch_optional(state.db())
    .await?;
    let (wh_url, wh_secret, smtp_url, smtp_from, smtp_to) =
        row.unwrap_or((None, None, None, None, None));
    Ok(Json(NotificationsConfig {
        webhook_url: wh_url,
        signing_enabled: wh_secret.is_some(),
        smtp_url,
        smtp_from,
        smtp_to,
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
    /// Email SMTP delivery. `None` keys leave the existing value
    /// untouched; empty strings clear; non-empty strings replace.
    #[serde(default)]
    pub smtp_url: Option<String>,
    #[serde(default)]
    pub smtp_from: Option<String>,
    #[serde(default)]
    pub smtp_to: Option<String>,
}

pub async fn put_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<PutBody>,
) -> AppResult<Json<NotificationsConfig>> {
    require_scope(&caller.scope, "tenant:admin")?;

    // Each field is partial-update: `None` leaves it alone, `Some("")`
    // clears, `Some("…")` replaces. Validate URL shape when set.
    let normalize = |o: Option<String>| -> Option<Option<String>> {
        o.map(|s| {
            let t = s.trim().to_string();
            if t.is_empty() { None } else { Some(t) }
        })
    };
    let wh_url = normalize(body.webhook_url);
    if let Some(Some(u)) = &wh_url {
        if !is_acceptable_url(u) {
            return Err(AppError::BadRequest(
                "webhook_url must be an http(s) URL".into(),
            ));
        }
    }
    let wh_secret = normalize(body.webhook_secret);
    let smtp_url = normalize(body.smtp_url);
    if let Some(Some(u)) = &smtp_url {
        if !(u.starts_with("smtp://") || u.starts_with("smtps://")) {
            return Err(AppError::BadRequest(
                "smtp_url must start with smtp:// or smtps://".into(),
            ));
        }
    }
    let smtp_from = normalize(body.smtp_from);
    let smtp_to = normalize(body.smtp_to);

    // CASE pattern: $N::int = 0 means "leave alone", else write the bind.
    sqlx::query(
        "UPDATE tenants SET \
            notifications_webhook_url    = CASE WHEN $2::int = 0 THEN notifications_webhook_url    ELSE $3 END, \
            notifications_webhook_secret = CASE WHEN $4::int = 0 THEN notifications_webhook_secret ELSE $5 END, \
            notification_smtp_url        = CASE WHEN $6::int = 0 THEN notification_smtp_url        ELSE $7 END, \
            notification_smtp_from       = CASE WHEN $8::int = 0 THEN notification_smtp_from       ELSE $9 END, \
            notification_smtp_to         = CASE WHEN $10::int = 0 THEN notification_smtp_to        ELSE $11 END, \
            updated_at = now() \
         WHERE id = $1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(if wh_url.is_some() { 1_i32 } else { 0 })
    .bind(wh_url.clone().unwrap_or(None))
    .bind(if wh_secret.is_some() { 1_i32 } else { 0 })
    .bind(wh_secret.clone().unwrap_or(None))
    .bind(if smtp_url.is_some() { 1_i32 } else { 0 })
    .bind(smtp_url.clone().unwrap_or(None))
    .bind(if smtp_from.is_some() { 1_i32 } else { 0 })
    .bind(smtp_from.clone().unwrap_or(None))
    .bind(if smtp_to.is_some() { 1_i32 } else { 0 })
    .bind(smtp_to.clone().unwrap_or(None))
    .execute(state.db())
    .await?;

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
                "webhook_url_changed": wh_url.is_some(),
                "webhook_secret_changed": wh_secret.is_some(),
                "smtp_url_changed": smtp_url.is_some(),
                "smtp_from_changed": smtp_from.is_some(),
                "smtp_to_changed": smtp_to.is_some(),
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
