//! Projects — curated skill/agent/command bundles for named repositories.
//!
//! A project lets a tenant admin assemble an exact set of items for a
//! specific project (e.g. "Acme Billing Service") so developers get exactly
//! those items when they run `skill-pool bootstrap --project acme-billing`.
//!
//! Route table:
//!
//! - `GET    /v1/tenant/projects`               — list (admin)
//! - `POST   /v1/tenant/projects`               — create (admin)
//! - `GET    /v1/tenant/projects/{slug}`         — detail with items (admin)
//! - `PATCH  /v1/tenant/projects/{slug}`         — update metadata (admin)
//! - `DELETE /v1/tenant/projects/{slug}`         — remove (admin)
//! - `PUT    /v1/tenant/projects/{slug}/items`   — replace items (admin)
//! - `GET    /v1/projects/resolve?remote=<url>` — resolve by git remote (any member)

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::admin::{self, ProjectPatch};
use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct ProjectResponse {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub git_remote: Option<String>,
    pub stack_tags: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
    /// Number of (skill | agent | command) items attached to this project.
    /// Populated by the list endpoint and by detail (where it equals
    /// `items.len()`). Lets the admin UI render an "Items" column without
    /// a second round-trip per row.
    pub item_count: i64,
}

#[derive(Serialize)]
pub struct ProjectItemResponse {
    pub skill_slug: String,
    pub kind: String,
    pub position: i32,
}

#[derive(Serialize)]
pub struct ProjectWithItemsResponse {
    #[serde(flatten)]
    pub project: ProjectResponse,
    pub items: Vec<ProjectItemResponse>,
}

#[derive(Serialize)]
pub struct ProjectRef {
    pub slug: String,
    pub name: String,
}

fn to_response(p: admin::Project) -> ProjectResponse {
    ProjectResponse {
        slug: p.slug,
        name: p.name,
        description: p.description,
        git_remote: p.git_remote,
        stack_tags: p.stack_tags,
        created_at: p.created_at,
        updated_at: p.updated_at,
        // The list endpoint overrides this via `to_response_with_count`;
        // the detail endpoint overrides it via `items.len()` below.
        // Defaulting to 0 keeps single-row callers (resolve, create) honest.
        item_count: 0,
    }
}

fn to_response_with_count(p: admin::Project, count: i64) -> ProjectResponse {
    let mut r = to_response(p);
    r.item_count = count;
    r
}

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct CreateBody {
    pub slug: String,
    pub name: String,
    pub description: Option<String>,
    pub git_remote: Option<String>,
}

#[derive(Deserialize)]
pub struct PatchBody {
    pub name: Option<String>,
    pub description: Option<Option<String>>,
    pub git_remote: Option<Option<String>>,
    pub stack_tags: Option<Vec<String>>,
    /// Toggle auto-refresh for this project's plan.
    /// `null` in JSON = clear (explicit-only). Integer = refresh every N seconds.
    /// Omitting the field = leave unchanged.
    pub plan_auto_refresh_interval_secs: Option<Option<i32>>,
}

#[derive(Deserialize)]
pub struct ItemInput {
    pub slug: String,
    pub kind: String,
}

#[derive(Deserialize)]
pub struct ResolveQuery {
    pub remote: String,
}

// ---------------------------------------------------------------------------
// Scope helper (mirrors stack_mappings.rs)
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
    // Any authenticated caller with at minimum `skills:read` is a tenant member.
    // The auth extractor already validated the tenant, so any non-empty scope
    // signals a legitimate member. We accept any scope here.
    if scope.is_empty() {
        Err(AppError::Forbidden)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

fn validate_slug(slug: &str) -> AppResult<&str> {
    let s = slug.trim();
    if s.is_empty() {
        return Err(AppError::BadRequest("slug is required".into()));
    }
    Ok(s)
}

fn validate_name(name: &str) -> AppResult<&str> {
    let n = name.trim();
    if n.is_empty() {
        return Err(AppError::BadRequest("name is required".into()));
    }
    Ok(n)
}

fn validate_kind(kind: &str) -> AppResult<()> {
    match kind {
        "skill" | "agent" | "command" | "plugin" => Ok(()),
        _ => Err(AppError::BadRequest(format!(
            "kind must be one of: skill, agent, command, plugin (got `{kind}`)"
        ))),
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `GET /v1/tenant/projects` — list all projects for the tenant (no items).
pub async fn list(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Json<Vec<ProjectResponse>>> {
    require_admin(&caller.scope)?;
    let projects = admin::list_projects_with_counts(state.db(), &caller.tenant.tenant_slug)
        .await
        .map_err(AppError::Anyhow)?;
    Ok(Json(
        projects
            .into_iter()
            .map(|(p, n)| to_response_with_count(p, n))
            .collect(),
    ))
}

/// `POST /v1/tenant/projects` — create a new project.
pub async fn create(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<CreateBody>,
) -> AppResult<(StatusCode, Json<ProjectResponse>)> {
    require_admin(&caller.scope)?;
    let slug = validate_slug(&body.slug)?;
    let name = validate_name(&body.name)?;

    let project = admin::create_project(
        state.db(),
        &caller.tenant.tenant_slug,
        slug,
        name,
        body.description.as_deref(),
        body.git_remote.as_deref(),
    )
    .await
    .map_err(|e| {
        // Surface unique-constraint violations as 409 Conflict. admin::create_project
        // wraps the underlying sqlx error with anyhow context, so e.to_string()
        // only renders the outer message ("create project ..."); use {:#} to walk
        // the chain and reach the postgres "duplicate key value violates unique
        // constraint" string.
        let msg = format!("{e:#}");
        if msg.contains("unique") || msg.contains("duplicate") {
            AppError::Conflict(format!("a project with slug `{slug}` already exists"))
        } else {
            AppError::Anyhow(e)
        }
    })?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "project.create",
            target_kind: "project",
            target_id: Some(slug),
            metadata: serde_json::json!({ "slug": slug, "name": name }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok((StatusCode::CREATED, Json(to_response(project))))
}

/// `GET /v1/tenant/projects/{slug}` — detail view with items.
pub async fn detail(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<Json<ProjectWithItemsResponse>> {
    require_admin(&caller.scope)?;

    let result = admin::get_project(state.db(), &caller.tenant.tenant_slug, &slug)
        .await
        .map_err(AppError::Anyhow)?
        .ok_or(AppError::NotFound)?;

    let items: Vec<ProjectItemResponse> = result
        .items
        .into_iter()
        .map(|i| ProjectItemResponse {
            skill_slug: i.skill_slug,
            kind: i.kind,
            position: i.position,
        })
        .collect();
    let item_count = items.len() as i64;
    Ok(Json(ProjectWithItemsResponse {
        project: to_response_with_count(result.project, item_count),
        items,
    }))
}

/// `PATCH /v1/tenant/projects/{slug}` — update metadata fields.
pub async fn patch(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Json(body): Json<PatchBody>,
) -> AppResult<Json<ProjectResponse>> {
    require_admin(&caller.scope)?;

    let patch = ProjectPatch {
        name: body.name,
        description: body.description,
        git_remote: body.git_remote,
        stack_tags: body.stack_tags,
        plan_auto_refresh_interval_secs: body.plan_auto_refresh_interval_secs,
    };

    let project = admin::update_project(state.db(), &caller.tenant.tenant_slug, &slug, patch)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound
            } else {
                AppError::Anyhow(e)
            }
        })?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "project.update",
            target_kind: "project",
            target_id: Some(slug.as_str()),
            metadata: serde_json::json!({ "slug": slug }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(Json(to_response(project)))
}

/// `DELETE /v1/tenant/projects/{slug}` — remove a project and its items.
pub async fn delete(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<StatusCode> {
    require_admin(&caller.scope)?;

    let deleted = admin::delete_project(state.db(), &caller.tenant.tenant_slug, &slug)
        .await
        .map_err(AppError::Anyhow)?;

    if !deleted {
        return Err(AppError::NotFound);
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "project.delete",
            target_kind: "project",
            target_id: Some(slug.as_str()),
            metadata: serde_json::json!({ "slug": slug }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// `PUT /v1/tenant/projects/{slug}/items` — atomically replace the project's
/// curated item list. The input order determines the `position` field.
pub async fn put_items(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Json(body): Json<Vec<ItemInput>>,
) -> AppResult<StatusCode> {
    require_admin(&caller.scope)?;

    // Validate all kinds before touching the database.
    for item in &body {
        validate_kind(&item.kind)?;
    }

    let items: Vec<(String, String)> = body.into_iter().map(|i| (i.slug, i.kind)).collect();

    admin::set_project_items(state.db(), &caller.tenant.tenant_slug, &slug, items)
        .await
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("not found") {
                AppError::NotFound
            } else {
                AppError::Anyhow(e)
            }
        })?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "project.set_items",
            target_kind: "project",
            target_id: Some(slug.as_str()),
            metadata: serde_json::json!({ "slug": slug }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/projects/resolve?remote=<url>` — look up a project by its
/// normalized git remote URL. Any authenticated tenant member may call this;
/// the CLI uses it to auto-discover the project slug from `git remote get-url origin`.
pub async fn resolve(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<ResolveQuery>,
) -> AppResult<Json<ProjectRef>> {
    require_member(&caller.scope)?;

    let remote = q.remote.trim().to_string();
    if remote.is_empty() {
        return Err(AppError::BadRequest(
            "remote query parameter is required".into(),
        ));
    }

    let project =
        admin::resolve_project_by_remote(state.db_read(), &caller.tenant.tenant_slug, &remote)
            .await
            .map_err(AppError::Anyhow)?
            .ok_or(AppError::NotFound)?;

    Ok(Json(ProjectRef {
        slug: project.slug,
        name: project.name,
    }))
}
