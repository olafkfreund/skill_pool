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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::cache;
use crate::error::AppError;
use crate::state::AppState;
use crate::tenant::TenantCtx;

/// How long a successful auth lookup is cached in Redis, in seconds.
///
/// 60 seconds is the deliberate sweet spot:
///   * Short enough that a token revoke or session revoke flips to 401
///     across the fleet within a minute, even when an admin forgets
///     to call the explicit invalidation hook.
///   * Long enough to absorb the typical per-request burst (CLI users,
///     dashboards on auto-refresh) — for steady-state traffic this
///     turns the per-request `SELECT FROM tenant_api_tokens` into one
///     query per token per minute.
///
/// **Misses are never cached.** A 401 right now might be a token that
/// will be minted ten seconds from now (CLI handing one off mid-script,
/// admin scripting a token-create followed immediately by an API call).
/// Caching the negative answer would convert a transient race into a
/// 60-second outage for legitimate callers.
const AUTH_CACHE_TTL_SECS: usize = 60;

/// Cached representation of a successful auth. Serialized as JSON in
/// Redis under `auth:v1:<sha256(raw_token)>`.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedAuth {
    /// Discriminator so we can tell API tokens from sessions when we
    /// reconstruct the `AuthedCaller`.
    kind: AuthKind,
    /// `token_id` from the table (api_tokens.id or sessions.id).
    token_id: Uuid,
    scope: String,
    user_id: Option<Uuid>,
    session_id: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum AuthKind {
    ApiToken,
    Session,
}

fn auth_cache_key(hashed: &str) -> String {
    // `hashed` is already SHA-256 of the raw token; safe to use as a
    // key — no leakage of the secret itself.
    format!("auth:v1:{hashed}")
}

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

        // Redis cache fast-path. A hit returns a fully-reconstructed
        // `AuthedCaller`. A miss (or any Redis error inside cache.rs)
        // falls through to the DB lookups below; we then write back
        // on success. Misses are NEVER cached — see AUTH_CACHE_TTL_SECS.
        if let Some(redis) = app.redis() {
            let key = auth_cache_key(&hashed);
            let mut conn = (**redis).clone();
            let cached: Option<String> = match redis::AsyncCommands::get(&mut conn, &key).await {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(error = %e, "redis GET (auth) failed; bypassing cache");
                    None
                }
            };
            if let Some(s) = cached {
                match serde_json::from_str::<CachedAuth>(&s) {
                    Ok(c) => {
                        // Bump last_used_at best-effort for API tokens,
                        // mirroring the uncached path. We skip the DB
                        // round-trip for session-based callers because
                        // user_sessions has no last_used_at column.
                        if matches!(c.kind, AuthKind::ApiToken) {
                            let _ = sqlx::query!(
                                "UPDATE tenant_api_tokens SET last_used_at = now() WHERE id = $1",
                                c.token_id,
                            )
                            .execute(app.db())
                            .await;
                        }
                        return Ok(AuthedCaller {
                            tenant,
                            token_id: c.token_id,
                            scope: c.scope,
                            user_id: c.user_id,
                            session_id: c.session_id,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "auth cache deserialize failed; refreshing");
                    }
                }
            }
        }

        // Try API token first — that's the CLI path and the common case.
        if let Some((token_id, scope)) = lookup_api_token(&app, tenant.tenant_id, &hashed).await? {
            let _ = sqlx::query!(
                "UPDATE tenant_api_tokens SET last_used_at = now() WHERE id = $1",
                token_id,
            )
            .execute(app.db())
            .await;
            // Best-effort cache write — never block the response on a
            // failed SETEX.
            if let Some(redis) = app.redis() {
                let entry = CachedAuth {
                    kind: AuthKind::ApiToken,
                    token_id,
                    scope: scope.clone(),
                    user_id: None,
                    session_id: None,
                };
                write_auth_cache(redis, &hashed, &entry).await;
            }
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
            let scope = role_to_scope(&role).to_string();
            if let Some(redis) = app.redis() {
                let entry = CachedAuth {
                    kind: AuthKind::Session,
                    token_id: session_id,
                    scope: scope.clone(),
                    user_id: Some(user_id),
                    session_id: Some(session_id),
                };
                write_auth_cache(redis, &hashed, &entry).await;
            }
            return Ok(AuthedCaller {
                tenant,
                token_id: session_id,
                scope,
                user_id: Some(user_id),
                session_id: Some(session_id),
            });
        }

        // Miss + DB-miss → 401. DO NOT cache; a token may be minted
        // moments from now.
        Err(AppError::Unauthorized)
    }
}

/// Best-effort cache write for a successful auth. Logs + ignores errors.
async fn write_auth_cache(redis: &cache::Redis, hashed: &str, entry: &CachedAuth) {
    let key = auth_cache_key(hashed);
    let payload = match serde_json::to_string(entry) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "auth cache serialize failed");
            return;
        }
    };
    let mut conn = (**redis).clone();
    let r: redis::RedisResult<()> = redis::AsyncCommands::set_ex(
        &mut conn,
        &key,
        payload,
        AUTH_CACHE_TTL_SECS as u64,
    )
    .await;
    if let Err(e) = r {
        tracing::warn!(error = %e, "auth cache SETEX failed");
    }
}

/// Invalidate the cached auth entry for a single raw token (after a
/// token-revoke or session-revoke). Caller hashes the raw token first.
#[allow(dead_code)] // wired by admin token-revoke once routed
pub async fn invalidate_token_cache(redis: &cache::Redis, hashed_token: &str) {
    let _ = cache::invalidate(redis, &auth_cache_key(hashed_token)).await;
}

/// Invalidate every cached auth entry. Use sparingly — called by
/// tenant-delete (which cascades all tokens + sessions) and by any
/// future "rotate all tokens" admin path.
#[allow(dead_code)] // wired by admin tenant-delete once routed
pub async fn invalidate_all_auth_cache(redis: &cache::Redis) {
    let _ = cache::invalidate_prefix(redis, "auth:v1:").await;
}

async fn lookup_api_token(
    state: &AppState,
    tenant_id: Uuid,
    hashed: &str,
) -> Result<Option<(Uuid, String)>, AppError> {
    let row = sqlx::query!(
        "SELECT id, scope FROM tenant_api_tokens \
         WHERE tenant_id = $1 AND hashed_token = $2 AND revoked_at IS NULL",
        tenant_id,
        hashed,
    )
    .fetch_optional(state.db())
    .await?;
    Ok(row.map(|r| (r.id, r.scope)))
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
    let rows = sqlx::query!(
        "SELECT role FROM tenant_role_mappings \
         WHERE tenant_id = $1 AND idp_group = ANY($2)",
        tenant_id,
        groups,
    )
    .fetch_all(db)
    .await?;
    if rows.is_empty() {
        return Ok(None);
    }
    let best = rows
        .into_iter()
        .map(|r| r.role)
        .max_by_key(|r| role_rank(r))
        .expect("non-empty after early return");

    sqlx::query!(
        "UPDATE tenant_users SET role = $1 WHERE tenant_id = $2 AND user_id = $3",
        best,
        tenant_id,
        user_id,
    )
    .execute(db)
    .await?;
    Ok(Some(best))
}

async fn lookup_session(
    state: &AppState,
    tenant_id: Uuid,
    hashed: &str,
) -> Result<Option<(Uuid, Uuid, String)>, AppError> {
    let row = sqlx::query!(
        "SELECT s.id, s.user_id, tu.role \
         FROM user_sessions s \
         JOIN tenant_users tu ON tu.tenant_id = s.tenant_id AND tu.user_id = s.user_id \
         WHERE s.tenant_id = $1 \
           AND s.hashed_token = $2 \
           AND s.revoked_at IS NULL \
           AND s.expires_at > now()",
        tenant_id,
        hashed,
    )
    .fetch_optional(state.db())
    .await?;
    Ok(row.map(|r| (r.id, r.user_id, r.role)))
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
