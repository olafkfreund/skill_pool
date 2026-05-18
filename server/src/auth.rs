//! API-token authentication. OIDC/SAML are layered on in Phase 2 (#4) for web sessions.

use axum::extract::{FromRef, FromRequestParts};
use axum::http::request::Parts;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::AppError;
use crate::state::AppState;
use crate::tenant::TenantCtx;

#[derive(Debug, Clone)]
#[allow(dead_code)] // fields consumed by handlers as endpoints fill in (#3)
pub struct AuthedCaller {
    pub tenant: TenantCtx,
    pub token_id: Uuid,
    pub scope: String,
}

pub fn hash_token(raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
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

        let row: Option<(Uuid, String)> = sqlx::query_as(
            "SELECT id, scope FROM tenant_api_tokens \
             WHERE tenant_id = $1 AND hashed_token = $2 AND revoked_at IS NULL",
        )
        .bind(tenant.tenant_id)
        .bind(&hashed)
        .fetch_optional(app.db())
        .await?;

        let (token_id, scope) = row.ok_or(AppError::Unauthorized)?;

        // Best-effort update of last_used_at; ignore errors.
        let _ = sqlx::query("UPDATE tenant_api_tokens SET last_used_at = now() WHERE id = $1")
            .bind(token_id)
            .execute(app.db())
            .await;

        Ok(AuthedCaller {
            tenant,
            token_id,
            scope,
        })
    }
}
