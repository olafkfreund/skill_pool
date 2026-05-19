//! Telemetry dashboards (Phase 5).
//!
//! - `GET /v1/tenant/usage/timeline?days=30` — admin only. Per-day
//!   buckets `[{ day, downloads, views, unique_skills }]` over the
//!   requested window. Missing days are filled with zeros so the chart
//!   doesn't have gaps.
//! - `GET /v1/tenant/usage/top?days=30&limit=10` — admin only.
//!   Most-active skills in the window, sorted by total events desc.
//! - `POST /v1/usage` — CLI-driven view event. `skill-pool ensure`
//!   posts one event per installed skill so the registry's decay model
//!   sees session-load activity, not just bundle downloads. Requires
//!   `skills:read`; rejects unknown slugs with 404.

use axum::extract::{Query, State};
use axum::http::StatusCode;
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
    .fetch_all(state.db_read())
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
    .fetch_all(state.db_read())
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

/// CLI-driven usage event. `skill-pool ensure` POSTs one of these per
/// installed skill so the decay model sees a real "session load" signal
/// alongside the existing server-side download/view events.
#[derive(Debug, Deserialize)]
pub struct UsageEventBody {
    pub skill_id: String,
    #[serde(default = "default_kind")]
    pub kind: String,
    pub event: String,
    /// SHA-256 (truncated to 16 hex chars) of the CLI's project root.
    /// Anonymises which project on which machine — we only need to
    /// dedup repeated events from the same install. Optional.
    #[serde(default)]
    pub project_hash: Option<String>,
}

fn default_kind() -> String {
    "skill".into()
}

/// `POST /v1/usage` — record a CLI-driven usage event.
///
/// Auth: requires `skills:read` (every CLI token has this). The body
/// names a slug + kind which we resolve to a `skills.id` for the
/// tenant; an unknown skill is a 404 so a stale manifest entry surfaces
/// instead of silently no-oping. Best-effort INSERT: the response
/// returns 202 once the row lands.
pub async fn post_event(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<UsageEventBody>,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "skills:read")?;

    // Only allow event kinds the schema's CHECK accepts. Adding new
    // kinds is a migration, not a free-form text field.
    let event_kind = match body.event.as_str() {
        "view" | "download" => body.event.as_str(),
        other => {
            return Err(AppError::BadRequest(format!(
                "event must be one of `view` or `download`, got `{other}`"
            )))
        }
    };
    let kind = match body.kind.as_str() {
        "skill" | "agent" | "command" => body.kind.as_str(),
        other => {
            return Err(AppError::BadRequest(format!(
                "kind must be one of `skill`, `agent`, `command`, got `{other}`"
            )))
        }
    };

    // Resolve slug → latest published `skills.id` for this tenant.
    // We require `published` here so a CLI hitting an archived slug
    // gets a 404 (the manifest is stale) rather than silently
    // recording activity against a graveyard row.
    let row: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&body.skill_id)
    .bind(kind)
    .fetch_optional(state.db_read())
    .await?;
    let (skill_id,) = row.ok_or(AppError::NotFound)?;

    // Best-effort: bump the per-row counter so decay sees this too,
    // then append the event row. A DB blip on either is logged but
    // doesn't fail the request — the CLI treats this as fire-and-forget.
    let r = sqlx::query(
        "UPDATE skills SET use_count = use_count + 1, last_used_at = now() WHERE id = $1",
    )
    .bind(skill_id)
    .execute(state.db())
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, skill_id = %skill_id, "use_count bump failed (CLI usage)");
    }

    let r = sqlx::query(
        "INSERT INTO skill_usage_events (tenant_id, skill_id, event_kind, user_id, token_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(caller.tenant.tenant_id)
    .bind(skill_id)
    .bind(event_kind)
    .bind(caller.user_id)
    .bind(caller.token_id)
    .execute(state.db())
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, skill_id = %skill_id, "CLI usage event insert failed");
        // We still return success — the event landing or not is a
        // background concern, the CLI shouldn't surface this to users.
    }

    let _ = body.project_hash; // accepted but not persisted in v1; see docs/lifecycle.md
    Ok(StatusCode::ACCEPTED)
}
