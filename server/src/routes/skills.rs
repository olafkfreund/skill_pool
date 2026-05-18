//! Skill endpoints. Phase 1 scaffold — full implementation tracked in issue #3.

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

#[derive(Serialize)]
pub struct Skill {
    pub slug: String,
    pub version: String,
    pub description: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[allow(dead_code)]
    pub query: Option<String>,
    #[allow(dead_code)]
    pub tags: Option<String>,
    #[allow(dead_code)]
    pub limit: Option<i64>,
}

pub async fn list(
    _state: State<AppState>,
    _tenant: TenantCtx,
    _query: axum::extract::Query<ListQuery>,
) -> AppResult<Json<Vec<Skill>>> {
    // TODO(#3): query DB filtered by tenant_id; honor `query`, `tags`, `limit`.
    Err(AppError::NotImplemented)
}

pub async fn get_one(
    _state: State<AppState>,
    _tenant: TenantCtx,
    Path(_slug): Path<String>,
) -> AppResult<Json<Skill>> {
    Err(AppError::NotImplemented)
}

pub async fn get_bundle(
    _state: State<AppState>,
    _tenant: TenantCtx,
    Path(_slug): Path<String>,
) -> AppResult<&'static str> {
    // TODO(#3): stream bundle from opendal storage; redirect to signed URL for S3.
    Err(AppError::NotImplemented)
}

pub async fn publish(_state: State<AppState>, _caller: AuthedCaller) -> AppResult<Json<Skill>> {
    // TODO(#3): accept multipart (metadata + bundle.tar.gz), validate, store, audit.
    Err(AppError::NotImplemented)
}

pub async fn validate(_state: State<AppState>, _caller: AuthedCaller) -> AppResult<&'static str> {
    // TODO(#3): lint-only path; reuse publish validators without persisting.
    Err(AppError::NotImplemented)
}
