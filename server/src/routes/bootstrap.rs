//! Bootstrap endpoint — given a stack fingerprint, return the curated skill
//! slugs the team has mapped to those tags.
//!
//! Phase 3 ships the curated-mapping tier (tenant admin maps tag → slug).
//! Tag intersection and embedding similarity come later — they layer on
//! the same response shape.

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const MAX_RESULTS: usize = 8;

#[derive(Deserialize)]
pub struct BootstrapQuery {
    /// Comma-separated stack tags.
    pub stack: String,
}

#[derive(Serialize)]
pub struct BootstrapResponse {
    /// Echoed tags actually used to look up mappings (post-normalisation).
    pub stack: Vec<String>,
    /// Recommended skill slugs in deterministic order.
    pub skills: Vec<String>,
}

pub async fn bootstrap(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<BootstrapQuery>,
) -> AppResult<Json<BootstrapResponse>> {
    let tags: Vec<String> = q
        .stack
        .split(',')
        .map(|t| t.trim().to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    if tags.is_empty() {
        return Err(AppError::BadRequest(
            "stack query must contain at least one comma-separated tag".into(),
        ));
    }

    // Curated mapping: every skill mapped to any of the user's stack tags,
    // deduped and capped. Order is deterministic (alphabetical) so two calls
    // return the same shape — the CLI's UI prompt stays stable across runs.
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT DISTINCT skill_slug \
         FROM tenant_stack_mappings \
         WHERE tenant_id = $1 AND stack_tag = ANY($2) \
         ORDER BY skill_slug \
         LIMIT $3",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&tags)
    .bind(MAX_RESULTS as i64)
    .fetch_all(state.db())
    .await?;

    Ok(Json(BootstrapResponse {
        stack: tags,
        skills: rows.into_iter().map(|(s,)| s).collect(),
    }))
}
