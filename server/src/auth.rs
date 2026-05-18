//! Authentication.
//!
//! Two token kinds, same SHA-256 storage:
//!   - API tokens (`tenant_api_tokens`) — used by CLI and machine-to-machine.
//!     Carry a free-form `scope` string (e.g. "skills:read skills:publish").
//!   - User sessions (`user_sessions`) — minted after an OIDC sign-in. Carry
//!     no explicit scope; their effective scope is derived from
//!     `tenant_users.role` and a fixed mapping.
//!
//! `AuthedCaller` records which kind authorised the request so handlers can
//! tell them apart (e.g. /v1/auth/whoami needs a user_id).

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::tenant::TenantCtx;

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct AuthedCaller {
    pub tenant: TenantCtx,
    pub token_id: Uuid,
    pub scope: String,
    pub user_id: Option<Uuid>,
    pub session_id: Option<Uuid>,
}

pub fn hash_token(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
}

/// Map a `tenant_users.role` to the equivalent scope string.
fn role_to_scope(role: &str) -> &'static str {
    match role {
        "admin" => "tenant:admin skills:read skills:publish",
        "curator" => "skills:read skills:publish",
        "publisher" => "skills:read skills:publish",
        // viewer (and anything unknown) — read-only.
        _ => "skills:read",
    }
}

impl<S> FromRequestParts<S> for AuthedCaller
where
    S: Send + Sync,
    AppState: FromRef<S>,
{
    type Rejection = AppError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let app = AppState::from_ref(state);
        let tenant = TenantCtx::from_request_parts(parts, state).await?;

        let raw = parts
            .headers
            .get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or(AppError::Unauthorized)?;
        let hashed = hash_token(raw);

        // Try API token first — that's the CLI path and the common case.
        if let Some((token_id, scope)) = lookup_api_token(&app, tenant.tenant_id, &hashed).await? {
            let _ = sqlx::query("UPDATE tenant_api_tokens SET last_used_at = now() WHERE id = $1")
                .bind(token_id)
                .execute(app.db())
                .await;
            return Ok(AuthedCaller {
                tenant,
                token_id,
                scope,
                user_id: None,
                session_id: None,
            });
        }

        // Fall through to session token (OIDC user).
        if let Some((session_id, user_id, role)) =
            lookup_session(&app, tenant.tenant_id, &hashed).await?
        {
            return Ok(AuthedCaller {
                tenant,
                token_id: session_id,
                scope: role_to_scope(&role).to_string(),
                user_id: Some(user_id),
                session_id: Some(session_id),
            });
        }

        Err(AppError::Unauthorized)
    }
}

async fn lookup_api_token(
    state: &AppState,
    tenant_id: Uuid,
    hashed: &str,
) -> Result<Option<(Uuid, String)>, AppError> {
    let row: Option<(Uuid, String)> = sqlx::query_as(
        "SELECT id, scope FROM tenant_api_tokens \
         WHERE tenant_id = $1 AND hashed_token = $2 AND revoked_at IS NULL",
    )
    .bind(tenant_id)
    .bind(hashed)
    .fetch_optional(state.db())
    .await?;
    Ok(row)
}

/// Role precedence: higher number wins. Used when an IdP-provided group set
/// matches multiple mappings (e.g. user is in both "Curators" and "Admins").
fn role_rank(role: &str) -> u8 {
    match role {
        "admin" => 3,
        "curator" => 2,
        "publisher" => 1,
        _ => 0, // viewer + anything unknown
    }
}

/// Apply IdP groups to a tenant_users row.
///
/// Returns `Ok(Some(role))` if a mapped group matched and the row was updated
/// to that role, `Ok(None)` if no groups matched (caller decides whether to
/// fall back to a default). Never downgrades a manual promotion when no
/// group claims match — preserves the existing role row.
pub async fn apply_role_from_groups(
    db: &sqlx::PgPool,
    tenant_id: Uuid,
    user_id: Uuid,
    groups: &[String],
) -> Result<Option<String>, AppError> {
    if groups.is_empty() {
        return Ok(None);
    }
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT role FROM tenant_role_mappings \
         WHERE tenant_id = $1 AND idp_group = ANY($2)",
    )
    .bind(tenant_id)
    .bind(groups)
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let best = rows
        .into_iter()
        .map(|(r,)| r)
        .max_by_key(|r| role_rank(r))
        .expect("non-empty after early return");

    sqlx::query("UPDATE tenant_users SET role = $1 WHERE tenant_id = $2 AND user_id = $3")
        .bind(&best)
        .bind(tenant_id)
        .bind(user_id)
        .execute(db)
        .await?;
    Ok(Some(best))
}

async fn lookup_session(
    state: &AppState,
    tenant_id: Uuid,
    hashed: &str,
) -> Result<Option<(Uuid, Uuid, String)>, AppError> {
    let row: Option<(Uuid, Uuid, String)> = sqlx::query_as(
        "SELECT s.id, s.user_id, tu.role \
         FROM user_sessions s \
         JOIN tenant_users tu ON tu.tenant_id = s.tenant_id AND tu.user_id = s.user_id \
         WHERE s.tenant_id = $1 \
           AND s.hashed_token = $2 \
           AND s.revoked_at IS NULL \
           AND s.expires_at > now()",
    )
    .bind(tenant_id)
    .bind(hashed)
    .fetch_optional(state.db())
    .await?;
    Ok(row)
}

#[cfg(test)]
mod tests {
    use super::role_rank;

    #[test]
    fn precedence_admin_beats_others() {
        assert!(role_rank("admin") > role_rank("curator"));
        assert!(role_rank("curator") > role_rank("publisher"));
        assert!(role_rank("publisher") > role_rank("viewer"));
        assert_eq!(role_rank("unknown"), role_rank("viewer"));
    }
}
