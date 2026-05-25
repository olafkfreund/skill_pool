//! Project Plans — per-project markdown documents imported from external sources.
//!
//! Plans are authored outside skill-pool (Confluence, Notion, GitHub, local files)
//! and imported by a curator. Each import creates an immutable version. The active
//! version is the source of truth for developers.
//!
//! Route table:
//!
//! - `POST   /v1/tenant/projects/{slug}/plan`            — import (admin)
//! - `GET    /v1/tenant/projects/{slug}/plan`            — active plan (any member)
//! - `GET    /v1/tenant/projects/{slug}/plan/versions`   — slim version list (any member)
//! - `GET    /v1/tenant/projects/{slug}/plan/versions/{v}` — specific version (any member)
//! - `POST   /v1/tenant/projects/{slug}/plan/refresh`    — re-fetch from source (admin)
//! - `POST   /v1/tenant/projects/{slug}/plan/activate`   — revert to version N (admin)
//! - The PATCH /v1/tenant/projects/{slug} auto-refresh toggle is handled in projects.rs

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::admin::{self, ImportPlanArgs, RefreshOutcome};
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct PlanResponse {
    pub id: uuid::Uuid,
    pub project_id: uuid::Uuid,
    pub version: i32,
    pub body_md: String,
    pub body_sha256: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub source_etag: Option<String>,
    pub imported_by: Option<uuid::Uuid>,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub status: String,
    pub fetch_error: Option<String>,
    pub fetch_error_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// Slim listing row — body is omitted to keep list responses compact.
#[derive(Serialize)]
pub struct PlanVersionSummary {
    pub id: uuid::Uuid,
    pub version: i32,
    pub body_sha256: String,
    pub source_type: String,
    pub source_url: Option<String>,
    pub imported_at: chrono::DateTime<chrono::Utc>,
    pub status: String,
    pub fetch_error: Option<String>,
    pub fetch_error_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[derive(Serialize)]
pub struct RefreshResponse {
    pub outcome: &'static str,
    pub version: Option<i32>,
}

fn to_response(p: admin::Plan) -> PlanResponse {
    PlanResponse {
        id: p.id,
        project_id: p.project_id,
        version: p.version,
        body_md: p.body_md,
        body_sha256: p.body_sha256,
        source_type: p.source_type,
        source_url: p.source_url,
        source_etag: p.source_etag,
        imported_by: p.imported_by,
        imported_at: p.imported_at,
        status: p.status,
        fetch_error: p.fetch_error,
        fetch_error_at: p.fetch_error_at,
    }
}

fn to_summary(p: admin::Plan) -> PlanVersionSummary {
    PlanVersionSummary {
        id: p.id,
        version: p.version,
        body_sha256: p.body_sha256,
        source_type: p.source_type,
        source_url: p.source_url,
        imported_at: p.imported_at,
        status: p.status,
        fetch_error: p.fetch_error,
        fetch_error_at: p.fetch_error_at,
    }
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

/// `POST /v1/tenant/projects/{slug}/plan`
#[derive(Deserialize)]
pub struct ImportBody {
    /// `"file"` or `"url"`.
    pub source_type: String,
    /// For `source_type = "url"`: the HTTPS URL to fetch. The server fetches
    /// the content and converts HTML→Markdown if needed.
    pub source_url: Option<String>,
    /// For `source_type = "file"`: the markdown body. Also accepted for
    /// `"url"` if the caller pre-fetched and just wants provenance tracking.
    pub body_md: Option<String>,
}

/// `POST /v1/tenant/projects/{slug}/plan/activate`
#[derive(Deserialize)]
pub struct ActivateBody {
    pub version: i32,
}

// ---------------------------------------------------------------------------
// Scope helpers (same pattern as projects.rs)
// ---------------------------------------------------------------------------

fn require_admin(scope: &str) -> AppResult<()> {
    if scope
        .split_whitespace()
        .any(|s| s == "tenant:admin" || s == "*")
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn require_member(scope: &str) -> AppResult<()> {
    if scope.is_empty() {
        Err(AppError::Forbidden)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/tenant/projects/{slug}/plan` — import a plan.
///
/// If `source_type = "url"`, the server fetches the URL (HTTPS only) and
/// converts HTML → Markdown if needed. If `source_type = "file"`, `body_md`
/// must be provided in the request body.
pub async fn import_plan(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Json(body): Json<ImportBody>,
) -> AppResult<(StatusCode, Json<PlanResponse>)> {
    require_admin(&caller.scope)?;

    let (body_md, source_url, etag) = match body.source_type.as_str() {
        "url" => {
            let url = body.source_url.as_deref().ok_or_else(|| {
                AppError::BadRequest("source_url is required when source_type is 'url'".into())
            })?;
            // If caller also provided body_md, use it (pre-fetched). Otherwise fetch.
            match body.body_md {
                Some(md) => (md, Some(url.to_owned()), None),
                None => {
                    let http = state.http_client();
                    let (md, etag) = admin::fetch_url_as_markdown(http, url)
                        .await
                        .map_err(|e| AppError::BadRequest(e.to_string()))?;
                    (md, Some(url.to_owned()), etag)
                }
            }
        }
        "file" => {
            let md = body.body_md.ok_or_else(|| {
                AppError::BadRequest("body_md is required when source_type is 'file'".into())
            })?;
            (md, body.source_url, None)
        }
        other => {
            return Err(AppError::BadRequest(format!(
                "source_type must be 'file' or 'url' (got `{other}`)"
            )));
        }
    };

    if body_md.is_empty() {
        return Err(AppError::BadRequest("plan body must not be empty".into()));
    }

    let plan = admin::import_plan(
        state.db(),
        ImportPlanArgs {
            tenant_slug: &caller.tenant.tenant_slug,
            project_slug: &slug,
            body_md: &body_md,
            source_type: &body.source_type,
            source_url: source_url.as_deref(),
            etag: etag.as_deref(),
            imported_by: caller.user_id,
        },
    )
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") {
            AppError::NotFound
        } else {
            AppError::Anyhow(e)
        }
    })?;

    Ok((StatusCode::CREATED, Json(to_response(plan))))
}

/// `GET /v1/tenant/projects/{slug}/plan` — active plan (full body).
pub async fn get_plan(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<Json<PlanResponse>> {
    require_member(&caller.scope)?;

    let plan = admin::get_active_plan(state.db_read(), &caller.tenant.tenant_slug, &slug)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound
            } else {
                AppError::Anyhow(e)
            }
        })?
        .ok_or(AppError::NotFound)?;

    Ok(Json(to_response(plan)))
}

/// `GET /v1/tenant/projects/{slug}/plan/versions` — slim history (no bodies).
pub async fn list_versions(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<Json<Vec<PlanVersionSummary>>> {
    require_member(&caller.scope)?;

    let plans = admin::list_plan_versions(state.db_read(), &caller.tenant.tenant_slug, &slug, 50)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound
            } else {
                AppError::Anyhow(e)
            }
        })?;

    Ok(Json(plans.into_iter().map(to_summary).collect()))
}

/// `GET /v1/tenant/projects/{slug}/plan/versions/{v}` — specific version (full body).
pub async fn get_version(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path((slug, version)): Path<(String, i32)>,
) -> AppResult<Json<PlanResponse>> {
    require_member(&caller.scope)?;

    // Fetch all versions (up to 1000) and find the requested one.
    // Using a targeted query here for correctness and simplicity.
    let plans = admin::list_plan_versions(state.db_read(), &caller.tenant.tenant_slug, &slug, 1000)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound
            } else {
                AppError::Anyhow(e)
            }
        })?;

    let plan = plans
        .into_iter()
        .find(|p| p.version == version)
        .ok_or(AppError::NotFound)?;

    Ok(Json(to_response(plan)))
}

/// `POST /v1/tenant/projects/{slug}/plan/refresh` — re-fetch from source.
///
/// Idempotent: if content unchanged or no source URL, returns `outcome: "unchanged"`.
/// If refresh fails, the active version is kept and `outcome: "failed"` is returned
/// with a 200 (the failure is recorded on the plan row).
pub async fn refresh_plan(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<Json<RefreshResponse>> {
    require_admin(&caller.scope)?;

    let outcome = admin::refresh_plan_from_source(
        state.db(),
        state.http_client(),
        &caller.tenant.tenant_slug,
        &slug,
    )
    .await
    .map_err(|e| {
        let msg = e.to_string();
        if msg.contains("not found") {
            AppError::NotFound
        } else {
            AppError::Anyhow(e)
        }
    })?;

    let resp = match outcome {
        RefreshOutcome::Unchanged => RefreshResponse {
            outcome: "unchanged",
            version: None,
        },
        RefreshOutcome::Updated(plan) => RefreshResponse {
            outcome: "updated",
            version: Some(plan.version),
        },
        RefreshOutcome::Failed(_) => RefreshResponse {
            outcome: "failed",
            version: None,
        },
    };

    Ok(Json(resp))
}

/// `POST /v1/tenant/projects/{slug}/plan/activate` — revert to a historical version.
pub async fn activate_version(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Json(body): Json<ActivateBody>,
) -> AppResult<Json<PlanResponse>> {
    require_admin(&caller.scope)?;

    let plan =
        admin::activate_plan_version(state.db(), &caller.tenant.tenant_slug, &slug, body.version)
            .await
            .map_err(|e| {
                let msg = e.to_string();
                if msg.contains("not found") {
                    AppError::NotFound
                } else {
                    AppError::Anyhow(e)
                }
            })?;

    Ok(Json(to_response(plan)))
}
