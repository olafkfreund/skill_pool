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

/// Strip the optional `:port` suffix and lowercase a Host header value.
/// Used by both the subdomain fallback in `slug_from_request` and the
/// custom-domain cache lookup in `TenantCtx`.
pub(crate) fn normalize_host(host: &str) -> String {
    host.split(':')
        .next()
        .unwrap_or(host)
        .trim()
        .to_lowercase()
}

impl<S> FromRequestParts<S> for TenantCtx
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let state = AppState::from_ref(state);

        // Custom-domain shortcut: in shared mode, before falling through
        // to the subdomain/header logic, check whether the exact Host
        // matches a verified/active row in `tenant_custom_domains` (via
        // the in-process cache populated by `AppState::refresh_custom_domains`).
        // A hit resolves directly to a tenant_id; we still load `slug`
        // for logging/audit.
        if matches!(state.tenancy(), crate::config::TenancyMode::Shared) {
            if let Some(host_raw) = parts.headers.get("host").and_then(|h| h.to_str().ok()) {
                let host = normalize_host(host_raw);
                if let Some(tenant_id) = state.custom_domain_tenant(&host).await {
                    let row = sqlx::query!(
                        "SELECT slug::text FROM tenants WHERE id = $1 AND status = 'active'",
                        tenant_id,
                    )
                    .fetch_optional(state.db())
                    .await?;
                    if let Some(r) = row {
                        // slug::text cast returns Option<String>; NOT NULL in schema.
                        let tenant_slug = r.slug.unwrap_or_default();
                        return Ok(TenantCtx {
                            tenant_id,
                            tenant_slug,
                        });
                    }
                    // Cache hit pointed at a tenant that's been suspended
                    // or deleted; fall through so subdomain logic handles
                    // it. The next refresh will purge the stale entry.
                }
            }
        }

        let slug = slug_from_request(parts, state.tenancy())?;

        let row = sqlx::query!(
            "SELECT id, slug::text FROM tenants WHERE slug = $1 AND status = 'active'",
            slug,
        )
        .fetch_optional(state.db())
        .await?;

        let r = row.ok_or(AppError::Unauthorized)?;
        // slug::text cast returns Option<String>; NOT NULL in schema.
        let tenant_slug = r.slug.unwrap_or_default();
        Ok(TenantCtx {
            tenant_id: r.id,
            tenant_slug,
        })
    }
}
