//! `GET /v1/tenant/profile/banner` — per-tenant CLI startup banner.
//!
//! No auth required. The CLI calls this from the user's shell before
//! they've necessarily authenticated, and the banner is policy not
//! secrets — knowing a tenant displays "Welcome to Acme" doesn't leak
//! anything sensitive (the tenant slug is already in the cookie / header
//! the CLI sends to reach this endpoint).
//!
//! Returns `{text: null, url: null}` when no banner is configured so the
//! CLI's "silently skip" path is just `text.is_none()`.
//!
//! See `docs/enterprise/branded-cli-banner.md` for the operator-facing doc.

use axum::extract::State;
use axum::Json;
use serde::Serialize;

use crate::error::AppResult;
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
    let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT banner_text, banner_url FROM tenants WHERE id = $1",
    )
    .bind(tenant.tenant_id)
    .fetch_optional(state.db_read())
    .await?;

    let (text, url) = row.unwrap_or((None, None));
    Ok(Json(TenantBanner { text, url }))
}
