//! Telemetry dashboards (Phase 5).
//!
//! - `GET /v1/tenant/usage/timeline?days=30` — admin only. Per-day
//!   buckets `[{ day, downloads, views, unique_skills }]` over the
//!   requested window. Missing days are filled with zeros so the chart
//!   doesn't have gaps.
//! - `GET /v1/tenant/usage/top?days=30&limit=10` — admin only.
//!   Most-active skills in the window, sorted by total events desc.

use axum::extract::{Query, State};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const DEFAULT_DAYS: i64 = 30;
const MAX_DAYS: i64 = 365;
const DEFAULT_TOP_LIMIT: i64 = 10;
const MAX_TOP_LIMIT: i64 = 100;

#[derive(Deserialize)]
pub struct WindowQuery {
    pub days: Option<i64>,
    pub limit: Option<i64>,
}

#[derive(Serialize, sqlx::FromRow)]
pub struct TimelineBucket {
    /// Truncated to start-of-day UTC.
    pub day: DateTime<Utc>,
    pub downloads: i64,
    pub views: i64,
    pub unique_skills: i64,
}

pub async fn timeline(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<WindowQuery>,
) -> AppResult<Json<Vec<TimelineBucket>>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let days = q.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);

    // generate_series fills missing days with zeros so the front-end
    // chart never has gaps. We round to UTC days; tenants in non-UTC
    // timezones will see "their day" shifted, which is fine for v1.
    let rows: Vec<TimelineBucket> = sqlx::query_as(
        "WITH days AS ( \
            SELECT generate_series( \
                date_trunc('day', now()) - ($1::int - 1) * INTERVAL '1 day', \
                date_trunc('day', now()), \
                INTERVAL '1 day' \
            ) AS day \
         ), \
         events AS ( \
            SELECT date_trunc('day', ts) AS day, event_kind, skill_id \
            FROM skill_usage_events \
            WHERE tenant_id = $2 \
              AND ts >= date_trunc('day', now()) - ($1::int - 1) * INTERVAL '1 day' \
         ) \
         SELECT \
            d.day, \
            COUNT(*) FILTER (WHERE e.event_kind = 'download') AS downloads, \
            COUNT(*) FILTER (WHERE e.event_kind = 'view')     AS views, \
            COUNT(DISTINCT e.skill_id)                        AS unique_skills \
         FROM days d \
         LEFT JOIN events e ON e.day = d.day \
         GROUP BY d.day \
         ORDER BY d.day ASC",
    )
    .bind(days as i32)
    .bind(caller.tenant.tenant_id)
    .fetch_all(state.db())
    .await?;

    Ok(Json(rows))
}

#[derive(Serialize, sqlx::FromRow)]
pub struct TopSkillRow {
    pub slug: String,
    pub downloads: i64,
    pub views: i64,
    pub total: i64,
}

pub async fn top(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<WindowQuery>,
) -> AppResult<Json<Vec<TopSkillRow>>> {
    require_scope(&caller.scope, "tenant:admin")?;
    let days = q.days.unwrap_or(DEFAULT_DAYS).clamp(1, MAX_DAYS);
    let limit = q.limit.unwrap_or(DEFAULT_TOP_LIMIT).clamp(1, MAX_TOP_LIMIT);

    let rows: Vec<TopSkillRow> = sqlx::query_as(
        "SELECT \
            s.slug, \
            COUNT(*) FILTER (WHERE e.event_kind = 'download') AS downloads, \
            COUNT(*) FILTER (WHERE e.event_kind = 'view')     AS views, \
            COUNT(*)                                          AS total \
         FROM skill_usage_events e \
         JOIN skills s ON s.id = e.skill_id \
         WHERE e.tenant_id = $1 \
           AND e.ts >= now() - ($2::int * INTERVAL '1 day') \
         GROUP BY s.slug \
         ORDER BY total DESC, s.slug ASC \
         LIMIT $3",
    )
    .bind(caller.tenant.tenant_id)
    .bind(days as i32)
    .bind(limit)
    .fetch_all(state.db())
    .await?;

    Ok(Json(rows))
}

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}
