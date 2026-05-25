//! Personal profile surface (#4).
//!
//! Two unrelated endpoints share this module because both are user-facing
//! shape, not tenant-admin shape:
//!
//!   * `GET /v1/tenant/profile/banner` — public per-tenant CLI startup banner.
//!     No auth required: the CLI calls this before the user has necessarily
//!     authenticated, and the banner is policy not secrets.
//!   * `GET/POST/DELETE /v1/profile/tokens` — personal API token management.
//!     Requires a session-authenticated caller (no `tenant:admin` scope
//!     needed; users always own their own tokens).
//!
//! ## Token surface invariants
//!
//! 1. POST returns the raw token *once*. The list endpoint never sees it
//!    again; only the SHA-256 hash sits in the DB, and the response shape
//!    deliberately omits `hashed_token`.
//! 2. Bare API-token callers (no `user_id`) are rejected with 401.
//!    A token cannot list/revoke "its own user's tokens" because tokens
//!    minted via the CLI carry no `created_by`. This keeps the surface
//!    strictly per-session-user.
//! 3. Revoke is idempotent: revoking an already-revoked token returns 204.
//!    A token id that doesn't belong to the caller returns 404 — we never
//!    leak "this id exists but isn't yours".

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::admin;
use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

#[derive(Serialize)]
pub struct TenantBanner {
    /// One-line greeting (≤240 chars). `null` when unset.
    pub text: Option<String>,
    /// Optional `https://` URL printed on the line below `text`. `null`
    /// when unset.
    pub url: Option<String>,
}

pub async fn get_banner(
    State(state): State<AppState>,
    tenant: TenantCtx,
) -> AppResult<Json<TenantBanner>> {
    let row = sqlx::query!(
        "SELECT banner_text, banner_url FROM tenants WHERE id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db_read())
    .await?;

    let (text, url) = row
        .map(|r| (r.banner_text, r.banner_url))
        .unwrap_or((None, None));
    Ok(Json(TenantBanner { text, url }))
}

// -- Personal API tokens ---------------------------------------------------

/// Scopes we let users mint for themselves. `tenant:admin` is included so
/// admins can produce machine credentials with the same set of capabilities
/// they already enjoy in the session — but the server still enforces that
/// non-admin callers can't pick `tenant:admin` (see `validate_scope`).
const ALLOWED_SCOPES: &[&str] = &["skills:read", "skills:publish", "tenant:admin"];

/// Wire shape for `GET /v1/profile/tokens`. Mirrors `admin::TokenSummary`
/// minus the hashed token, with snake_case field names to match the rest
/// of the API surface.
#[derive(Serialize)]
pub struct TokenView {
    pub id: Uuid,
    /// User-supplied label (column `name`).
    pub label: String,
    /// First ~12 chars of the raw token. `null` for tokens minted before
    /// migration 0028 (or via the legacy CLI path).
    pub prefix: Option<String>,
    /// Space-separated scope string, exactly as stored.
    pub scopes: String,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl From<admin::TokenSummary> for TokenView {
    fn from(t: admin::TokenSummary) -> Self {
        Self {
            id: t.id,
            label: t.name,
            prefix: t.prefix,
            scopes: t.scope,
            created_at: t.created_at,
            last_used_at: t.last_used_at,
            revoked_at: t.revoked_at,
        }
    }
}

pub async fn list_tokens(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<Vec<TokenView>>> {
    let user_id = require_user(&caller)?;
    let rows = admin::list_user_tokens(state.db(), &caller.tenant.tenant_slug, user_id).await?;
    Ok(Json(rows.into_iter().map(TokenView::from).collect()))
}

#[derive(Deserialize)]
pub struct CreateBody {
    /// Display label for the token. Trimmed; must be 1–80 chars.
    pub label: String,
    /// Either a space-separated string or a list of strings — both shapes
    /// are friendlier than forcing the UI to pick one.
    #[serde(default)]
    pub scopes: ScopeInput,
}

/// Tolerant input shape so the UI can post `["skills:read", ...]` while a
/// curl one-liner can post `"skills:read skills:publish"`.
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ScopeInput {
    #[default]
    Empty,
    List(Vec<String>),
    Joined(String),
}

impl ScopeInput {
    fn into_canonical(self) -> Vec<String> {
        match self {
            Self::Empty => Vec::new(),
            Self::List(v) => v
                .into_iter()
                .flat_map(|s| s.split_whitespace().map(str::to_string).collect::<Vec<_>>())
                .collect(),
            Self::Joined(s) => s.split_whitespace().map(str::to_string).collect(),
        }
    }
}

#[derive(Serialize)]
pub struct CreatedTokenView {
    pub id: Uuid,
    /// **Shown once.** The caller must store this; we only retain its hash.
    pub raw_token: String,
    pub prefix: String,
    pub scopes: String,
    pub created_at: DateTime<Utc>,
    pub label: String,
}

pub async fn create_token(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<CreateBody>,
) -> AppResult<Json<CreatedTokenView>> {
    let user_id = require_user(&caller)?;

    let label = body.label.trim();
    if label.is_empty() || label.chars().count() > 80 {
        return Err(AppError::BadRequest("label must be 1–80 characters".into()));
    }

    let scopes_vec = body.scopes.into_canonical();
    if scopes_vec.is_empty() {
        return Err(AppError::BadRequest(
            "at least one scope is required".into(),
        ));
    }
    validate_scopes(&scopes_vec, &caller.scope)?;
    let scope_str = scopes_vec.join(" ");

    let created = admin::create_user_token(
        state.db(),
        &caller.tenant.tenant_slug,
        label,
        &scope_str,
        user_id,
    )
    .await
    .map_err(AppError::Anyhow)?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "profile.token.create",
            target_kind: "tenant_api_token",
            target_id: Some(&created.id.to_string()),
            metadata: serde_json::json!({
                "label": label,
                "scopes": scope_str,
                "prefix": created.prefix,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(Json(CreatedTokenView {
        id: created.id,
        raw_token: created.raw_token,
        prefix: created.prefix,
        scopes: scope_str,
        created_at: created.created_at,
        label: label.to_string(),
    }))
}

pub async fn revoke_token(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(token_id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let user_id = require_user(&caller)?;

    let existed =
        admin::revoke_user_token(state.db(), &caller.tenant.tenant_slug, user_id, token_id)
            .await
            .map_err(AppError::Anyhow)?;
    if !existed {
        return Err(AppError::NotFound);
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "profile.token.revoke",
            target_kind: "tenant_api_token",
            target_id: Some(&token_id.to_string()),
            metadata: serde_json::json!({}),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// -- helpers ---------------------------------------------------------------

fn require_user(caller: &AuthedCaller) -> AppResult<Uuid> {
    caller.user_id.ok_or(AppError::Unauthorized)
}

fn caller_has(scope_str: &str, needed: &str) -> bool {
    scope_str
        .split_whitespace()
        .any(|s| s == needed || s == "*")
}

fn validate_scopes(scopes: &[String], caller_scope: &str) -> AppResult<()> {
    for s in scopes {
        if !ALLOWED_SCOPES.contains(&s.as_str()) {
            return Err(AppError::BadRequest(format!(
                "unknown scope `{s}`; allowed: {}",
                ALLOWED_SCOPES.join(", ")
            )));
        }
        // A non-admin caller cannot mint a token with admin scope. Without
        // this rule a publisher could escalate to tenant:admin by minting
        // themselves a fresh credential.
        if s == "tenant:admin" && !caller_has(caller_scope, "tenant:admin") {
            return Err(AppError::Forbidden);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_input_accepts_list_and_string() {
        let list = ScopeInput::List(vec!["skills:read".into(), "skills:publish".into()]);
        assert_eq!(
            list.into_canonical(),
            vec!["skills:read".to_string(), "skills:publish".into()]
        );

        let joined = ScopeInput::Joined("skills:read skills:publish".into());
        assert_eq!(
            joined.into_canonical(),
            vec!["skills:read".to_string(), "skills:publish".into()]
        );

        // Whitespace-bearing list entries get split too — covers a UI that
        // accidentally posts `["skills:read skills:publish"]`.
        let messy = ScopeInput::List(vec!["skills:read skills:publish".into()]);
        assert_eq!(
            messy.into_canonical(),
            vec!["skills:read".to_string(), "skills:publish".into()]
        );

        assert!(ScopeInput::Empty.into_canonical().is_empty());
    }

    #[test]
    fn validate_scopes_blocks_admin_escalation() {
        let admin_caller = "tenant:admin skills:read skills:publish";
        let publisher_caller = "skills:read skills:publish";

        // Admin can mint admin.
        assert!(validate_scopes(&["tenant:admin".into()], admin_caller).is_ok());

        // Publisher cannot mint admin.
        let err = validate_scopes(&["tenant:admin".into()], publisher_caller).unwrap_err();
        assert!(matches!(err, AppError::Forbidden));

        // Anyone can mint read scope.
        assert!(validate_scopes(&["skills:read".into()], publisher_caller).is_ok());
    }

    #[test]
    fn validate_scopes_rejects_unknown() {
        let err = validate_scopes(
            &["skills:read".into(), "rogue:scope".into()],
            "tenant:admin",
        )
        .unwrap_err();
        match err {
            AppError::BadRequest(msg) => assert!(msg.contains("rogue:scope"), "{msg}"),
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }
}
