//! Decay lifecycle (Phase 5).
//!
//! - `GET  /v1/tenant/skills/decay?days=180&max_uses=3` — admin only.
//!   Lists published skills whose `last_used_at` is older than `days`
//!   AND whose `use_count` is below `max_uses`. The master plan's
//!   default heuristic: 6 months + < 3 uses.
//! - `POST /v1/skills/{slug}/archive` — admin only. Flips the latest
//!   version's `status` to `archived`. Catalog list filters
//!   `status='published'` so archived skills auto-hide.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const DEFAULT_DAYS: i64 = 180;
const DEFAULT_MAX_USES: i32 = 3;
const MAX_DAYS: i64 = 365 * 5;
const MAX_MAX_USES: i32 = 100;

#[derive(Deserialize)]
pub struct DecayQuery {
    pub days: Option<i64>,
    pub max_uses: Option<i32>,
    pub limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct DecayCandidate {
    pub slug: String,
    pub version: String,
    pub description: String,
    pub use_count: i32,
    pub last_used_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

pub async fn list_candidates(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<DecayQuery>,
) -> AppResult<Json<Vec<DecayCandidate>>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let days = q.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
    let max_uses = q.max_uses.unwrap_or(DEFAULT_MAX_USES).clamp(0, MAX_MAX_USES);
    let limit = q.limit.unwrap_or(200).clamp(1, 1000);

    // CTE picks the latest published version per slug, then filters by
    // staleness on that result. Mirrors the list endpoint's semantics.
    // Skills-only by default for v1. Agents and commands would need
    // their own decay tuning (different baseline usage); revisit when
    // those kinds have meaningful traffic.
    let rows: Vec<DecayCandidate> = sqlx::query_as(
        "WITH latest AS ( \
           SELECT DISTINCT ON (slug) \
             slug, version, description, use_count, last_used_at, created_at \
           FROM skills \
           WHERE tenant_id = $1 AND kind = 'skill' AND status = 'published' \
           ORDER BY slug, created_at DESC \
         ) \
         SELECT slug, version, description, use_count, last_used_at, created_at \
         FROM latest \
         WHERE use_count < $2 \
           AND (last_used_at IS NULL OR last_used_at < now() - make_interval(days => $3::int)) \
         ORDER BY last_used_at ASC NULLS FIRST, use_count ASC \
         LIMIT $4",
    )
    .bind(caller.tenant.tenant_id)
    .bind(max_uses)
    .bind(days as i32)
    .bind(limit)
    .fetch_all(state.db())
    .await?;

    Ok(Json(rows))
}

#[derive(Serialize)]
pub struct ArchiveResponse {
    pub slug: String,
    pub version: String,
}

#[derive(Deserialize)]
pub struct ArchiveQuery {
    pub kind: Option<String>,
}

pub async fn archive(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Query(q): Query<ArchiveQuery>,
) -> AppResult<Json<ArchiveResponse>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let kind = match q.kind.as_deref().unwrap_or("skill") {
        k @ ("skill" | "agent" | "command") => k,
        other => {
            return Err(AppError::BadRequest(format!(
                "kind must be skill/agent/command, got `{other}`"
            )))
        }
    };

    // Flip the latest published version of this slug + kind.
    let row: Option<(String,)> = sqlx::query_as(
        "UPDATE skills SET status = 'archived' \
         WHERE id = ( \
            SELECT id FROM skills \
            WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
            ORDER BY created_at DESC LIMIT 1 \
         ) \
         RETURNING version",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&slug)
    .bind(kind)
    .fetch_optional(state.db())
    .await?;

    let (version,) = row.ok_or(AppError::NotFound)?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "skill.archive",
            target_kind: "skill",
            target_id: Some(&slug),
            metadata: serde_json::json!({ "version": version }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(Json(ArchiveResponse { slug, version }))
}

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

// keep StatusCode in scope for crate compat with the existing module pattern
#[allow(dead_code)]
const _: StatusCode = StatusCode::OK;
