//! Tenant resolution.
//!
//! The Axum extractor reads the `Host` header (subdomain → slug) in shared mode,
//! or pins the slug in dedicated mode. The middleware then resolves the slug to a
//! `tenant_id` against the database. Every handler that touches tenant data MUST
//! take a `TenantCtx` extractor — by convention enforced in code review and
//! eventually by a build-time lint (see issue #8).

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use uuid::Uuid;

use crate::config::TenancyMode;
use crate::error::AppError;
use crate::state::AppState;

#[derive(Debug, Clone)]
#[allow(dead_code)] // tenant_slug used by logging/audit once wired (#3)
pub struct TenantCtx {
    pub tenant_id: Uuid,
    pub tenant_slug: String,
}

/// Extract the tenant slug from the request without hitting the database.
pub fn slug_from_request(parts: &Parts, tenancy: &TenancyMode) -> Result<String, AppError> {
    match tenancy {
        TenancyMode::Dedicated { tenant_slug } => Ok(tenant_slug.clone()),
        TenancyMode::Shared => {
            // Prefer explicit header (testing, CI, dev) over subdomain.
            if let Some(h) = parts.headers.get("x-skill-pool-tenant") {
                let s = h
                    .to_str()
                    .map_err(|_| AppError::TenantResolution("invalid header".into()))?
                    .trim()
                    .to_lowercase();
                if !s.is_empty() {
                    return Ok(s);
                }
            }

            let host = parts
                .headers
                .get("host")
                .and_then(|h| h.to_str().ok())
                .ok_or_else(|| AppError::TenantResolution("missing Host header".into()))?;

            // Strip port, take the leading label as the slug.
            let host_no_port = host.split(':').next().unwrap_or(host);
            let mut parts_iter = host_no_port.split('.');
            let slug = parts_iter
                .next()
                .filter(|s| !s.is_empty() && *s != "www")
                .ok_or_else(|| AppError::TenantResolution("no subdomain in host".into()))?;

            Ok(slug.to_lowercase())
        }
    }
}

impl<S> FromRequestParts<S> for TenantCtx
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);
        let slug = slug_from_request(parts, state.tenancy())?;

        let row: Option<(Uuid, String)> =
            sqlx::query_as("SELECT id, slug FROM tenants WHERE slug = $1 AND status = 'active'")
                .bind(&slug)
                .fetch_optional(state.db())
                .await?;

        let (tenant_id, tenant_slug) = row.ok_or(AppError::Unauthorized)?;
        Ok(TenantCtx {
            tenant_id,
            tenant_slug,
        })
    }
}
