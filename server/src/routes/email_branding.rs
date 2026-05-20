//! Per-tenant branded email config surface (#9).
//!
//! Sits at `/v1/tenant/email-branding`. All endpoints require the
//! `tenant:admin` scope. The password is encrypted at rest with
//! AES-256-GCM (see `email_branding::encrypt_password`) and is never
//! returned by GET — clients see only `password_configured: bool`.
//!
//! Why a separate surface from `/v1/tenant/notifications`:
//! the notifications endpoint configures *what gets sent* (digest
//! recipient, webhook URL); this one configures *how the From line and
//! transport look*. Many enterprise tenants will use only one of them.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::email_branding::{self, BrandingRow};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

#[derive(Serialize)]
pub struct BrandingView {
    pub from_addr: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<String>,
    /// The raw URL with the password stripped out of the userinfo.
    /// Stored URLs already lack the password (we store it encrypted in
    /// a separate column), so this is the same value as stored.
    pub smtp_url: String,
    /// Whether the encrypted-password column holds something. Never
    /// leak the password itself.
    pub password_configured: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub footer_html: Option<String>,
}

impl From<&BrandingRow> for BrandingView {
    fn from(r: &BrandingRow) -> Self {
        Self {
            from_addr: r.from_addr.clone(),
            from_name: r.from_name.clone(),
            reply_to: r.reply_to.clone(),
            smtp_url: r.smtp_url.clone(),
            password_configured: !r.smtp_password_enc.is_empty(),
            footer_html: r.footer_html.clone(),
        }
    }
}

pub async fn get_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<BrandingView>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let row = email_branding::load_row(state.db(), caller.tenant.tenant_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(BrandingView::from(&row)))
}

#[derive(Deserialize)]
pub struct PutBody {
    pub from_addr: String,
    #[serde(default)]
    pub from_name: Option<String>,
    #[serde(default)]
    pub reply_to: Option<String>,
    pub smtp_url: String,
    pub smtp_password: String,
    #[serde(default)]
    pub footer_html: Option<String>,
}

pub async fn put_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<PutBody>,
) -> AppResult<Json<BrandingView>> {
    require_scope(&caller.scope, "tenant:admin")?;

    if !email_branding::looks_like_email(body.from_addr.trim()) {
        return Err(AppError::BadRequest(
            "from_addr must be a valid email address".into(),
        ));
    }
    if let Some(rt) = &body.reply_to {
        if !rt.trim().is_empty() && !email_branding::looks_like_email(rt.trim()) {
            return Err(AppError::BadRequest(
                "reply_to must be a valid email address or omitted".into(),
            ));
        }
    }
    let smtp_url = body.smtp_url.trim().to_string();
    if !(smtp_url.starts_with("smtp://") || smtp_url.starts_with("smtps://")) {
        return Err(AppError::BadRequest(
            "smtp_url must start with smtp:// or smtps://".into(),
        ));
    }
    if body.smtp_password.is_empty() {
        return Err(AppError::BadRequest("smtp_password must not be empty".into()));
    }

    let enc = email_branding::encrypt_password(&body.smtp_password);

    sqlx::query!(
        "INSERT INTO tenant_email_branding \
            (tenant_id, from_addr, from_name, reply_to, smtp_url, smtp_password_enc, footer_html, updated_at) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, now()) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            from_addr = EXCLUDED.from_addr, \
            from_name = EXCLUDED.from_name, \
            reply_to = EXCLUDED.reply_to, \
            smtp_url = EXCLUDED.smtp_url, \
            smtp_password_enc = EXCLUDED.smtp_password_enc, \
            footer_html = EXCLUDED.footer_html, \
            updated_at = now()",
        caller.tenant.tenant_id,
        body.from_addr.trim(),
        body.from_name.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        body.reply_to.as_deref().map(str::trim).filter(|s| !s.is_empty()),
        &smtp_url,
        &enc as &[u8],
        body.footer_html.as_deref().filter(|s| !s.is_empty()),
    )
    .execute(state.db())
    .await?;

    // Bust the cache so the next send picks up the new password/URL.
    state
        .email_transport()
        .invalidate(caller.tenant.tenant_id)
        .await;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.email_branding.update",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({
                "from_addr": body.from_addr.trim(),
                "smtp_url": smtp_url,
                "has_reply_to": body.reply_to.as_deref().is_some_and(|s| !s.trim().is_empty()),
                "has_footer_html": body.footer_html.as_deref().is_some_and(|s| !s.is_empty()),
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    // Re-load + return the canonical view (with password masked).
    let row = email_branding::load_row(state.db(), caller.tenant.tenant_id)
        .await?
        .ok_or(AppError::NotFound)?;
    Ok(Json(BrandingView::from(&row)))
}

pub async fn delete_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "tenant:admin")?;
    sqlx::query!(
        "DELETE FROM tenant_email_branding WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .execute(state.db())
    .await?;
    state
        .email_transport()
        .invalidate(caller.tenant.tenant_id)
        .await;
    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.email_branding.delete",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({}),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
pub struct TestBody {
    pub recipient: String,
}

#[derive(Serialize)]
pub struct TestResult {
    pub result: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Send a one-line test email through the tenant's branded transport.
/// Useful for verifying SMTP config before relying on it for digests.
/// Always returns 200; the JSON body's `result` field is `success` or
/// `failed`. We intentionally don't surface a 5xx on send failure
/// because the SMTP server reachability is the user's problem to
/// diagnose, not a server bug.
pub async fn test_config(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<TestBody>,
) -> AppResult<Json<TestResult>> {
    require_scope(&caller.scope, "tenant:admin")?;
    if !email_branding::looks_like_email(body.recipient.trim()) {
        return Err(AppError::BadRequest(
            "recipient must be a valid email address".into(),
        ));
    }
    let row = email_branding::load_row(state.db(), caller.tenant.tenant_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("no email branding configured".into()))?;

    let subject = format!(
        "[skill-pool] Branded-email test for tenant {}",
        caller.tenant.tenant_slug
    );
    let send_body = format!(
        "This is a test message confirming that branded transactional email is wired \
         correctly for `{}`. If you received this, your SMTP transport and From address \
         are working as expected.\n",
        caller.tenant.tenant_slug
    );

    let outcome = email_branding::send_branded(
        state.email_transport().as_ref(),
        &row,
        body.recipient.trim(),
        &subject,
        &send_body,
    )
    .await;

    let (result, error) = match outcome {
        email_branding::SendOutcome::Success => ("success", None),
        email_branding::SendOutcome::Failed(e) => ("failed", Some(e)),
    };

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "tenant.email_branding.test",
            target_kind: "tenant",
            target_id: Some(caller.tenant.tenant_slug.as_str()),
            metadata: serde_json::json!({
                "result": result,
                "to": body.recipient.trim(),
                "error": error,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(Json(TestResult { result, error }))
}

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
