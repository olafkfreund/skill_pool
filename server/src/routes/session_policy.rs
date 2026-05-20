//! `GET /v1/tenant/session-policy` — per-tenant session settings the web
//! portal needs at login time.
//!
//! No auth required. The login page calls this before the user has a
//! token to know what `maxAge` to put on the session cookie. The values
//! here are policy not secrets — knowing "this tenant's sessions expire
//! in 1 hour" leaks nothing.
//!
//! See `docs/enterprise/session-policy.md` for the operator-facing doc.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::error::AppResult;
use crate::state::AppState;
use crate::tenant::TenantCtx;

/// 14 days. Matches the hardcoded fallback the SvelteKit login action
/// has used since Phase 2. Returned when `tenants.session_max_age_secs IS NULL`.
const DEFAULT_SESSION_MAX_AGE_SECS: i32 = 14 * 24 * 60 * 60;

#[derive(Serialize)]
pub struct SessionPolicy {
    /// Session cookie maxAge in seconds. The web portal applies this to
    /// `sp_token` and `sp_tenant` cookies at login.
    pub max_age_secs: i32,
    /// `true` when this tenant has a custom policy (the column is set);
    /// `false` when the response is the default fallback. Useful for
    /// admin UIs that want to show "(default)" next to the value.
    pub configured: bool,
}

pub async fn get_session_policy(
    State(state): State<AppState>,
    tenant: TenantCtx,
) -> AppResult<Json<SessionPolicy>> {
    let row = sqlx::query!(
        "SELECT session_max_age_secs FROM tenants WHERE id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db_read())
    .await?;

    let configured = matches!(row, Some(ref r) if r.session_max_age_secs.is_some());
    let max_age_secs = row
        .and_then(|r| r.session_max_age_secs)
        .unwrap_or(DEFAULT_SESSION_MAX_AGE_SECS);

    Ok(Json(SessionPolicy {
        max_age_secs,
        configured,
    }))
}
