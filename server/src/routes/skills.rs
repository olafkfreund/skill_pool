//! Skill endpoints — Phase 1 implementation.
//!
//! - `GET    /v1/skills`                    list
//! - `GET    /v1/skills/{slug}`             metadata for the latest version
//! - `GET    /v1/skills/{slug}/bundle.tar.gz`  bundle stream (or 302 to signed URL)
//! - `POST   /v1/skills`                    publish (multipart: bundle + metadata JSON)
//! - `POST   /v1/skills/validate`           lint without persisting

use std::collections::HashMap;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::Json;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::bundle::{self, BundleError};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::storage::Storage;
use crate::tenant::TenantCtx;

const MAX_LIMIT: i64 = 200;
const DEFAULT_LIMIT: i64 = 50;

#[derive(Serialize)]
pub struct Skill {
    pub slug: String,
    pub version: String,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<String>,
    pub tags: Vec<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    /// Cosine similarity to the `semantic` query, if one was supplied.
    /// Absent on plain list / keyword responses so the shape stays
    /// byte-identical with the pre-Phase-5 API for default clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    pub query: Option<String>,
    /// Comma-separated tag list. ALL must match.
    pub tags: Option<String>,
    pub limit: Option<i64>,
    /// When set, rank results by cosine similarity of `description_embedding`
    /// to this query string. Requires an embedder configured on the server
    /// (`--features fastembed`). Coexists with `tags` (both apply).
    pub semantic: Option<String>,
    /// Minimum similarity (0.0..=1.0) when `semantic` is set. Defaults to 0.0
    /// — return all matches ordered by similarity.
    pub min_similarity: Option<f32>,
}

pub async fn list(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<Skill>>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let tag_list: Vec<String> = q
        .tags
        .as_deref()
        .map(|s| {
            s.split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    // --- Semantic-ranked branch -----------------------------------------
    if let Some(query_text) = q.semantic.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        let embedding = state
            .embedder()
            .embed(query_text)
            .map_err(AppError::Anyhow)?;
        let Some(vec) = embedding else {
            return Err(AppError::BadRequest(
                "semantic search is not enabled on this server (no embedder configured)".into(),
            ));
        };
        let lit = crate::embedding::vector_to_pg_literal(&vec);
        let min_sim = q.min_similarity.unwrap_or(0.0).clamp(0.0, 1.0);

        // CTE picks the latest published version per slug first (preserving
        // the existing list semantics), then ranks by cosine similarity over
        // that result. The HNSW index on description_embedding still helps
        // because the planner can push the <=> ordering down.
        let rows: Vec<SkillRowSemantic> = sqlx::query_as(
            "WITH latest AS ( \
               SELECT DISTINCT ON (slug) \
                 slug, version, description, when_to_use, tags, status, created_at, \
                 description_embedding \
               FROM skills \
               WHERE tenant_id = $1 \
                 AND status = 'published' \
                 AND ($2::text[] = '{}' OR tags @> $2) \
                 AND description_embedding IS NOT NULL \
               ORDER BY slug, created_at DESC \
             ) \
             SELECT slug, version, description, when_to_use, tags, status, created_at, \
                    (1 - (description_embedding <=> $3::text::vector))::real AS similarity \
             FROM latest \
             WHERE (1 - (description_embedding <=> $3::text::vector))::real >= $4 \
             ORDER BY similarity DESC \
             LIMIT $5",
        )
        .bind(tenant.tenant_id)
        .bind(&tag_list)
        .bind(lit)
        .bind(min_sim)
        .bind(limit)
        .fetch_all(state.db())
        .await?;

        return Ok(Json(rows.into_iter().map(Into::into).collect()));
    }

    // --- Keyword / tag / plain-list branch (unchanged) ------------------
    let needle = q.query.as_deref().map(|s| format!("%{s}%"));
    let rows: Vec<SkillRow> = sqlx::query_as(
        "SELECT DISTINCT ON (slug) \
            slug, version, description, when_to_use, tags, status, created_at \
         FROM skills \
         WHERE tenant_id = $1 \
           AND status = 'published' \
           AND ($2::text IS NULL OR description ILIKE $2 OR slug ILIKE $2) \
           AND ($3::text[] = '{}' OR tags @> $3) \
         ORDER BY slug, created_at DESC \
         LIMIT $4",
    )
    .bind(tenant.tenant_id)
    .bind(needle)
    .bind(&tag_list)
    .bind(limit)
    .fetch_all(state.db())
    .await?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

pub async fn get_one(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug): Path<String>,
) -> AppResult<Json<Skill>> {
    let row: Option<SkillRow> = sqlx::query_as(
        "SELECT slug, version, description, when_to_use, tags, status, created_at \
         FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(&slug)
    .fetch_optional(state.db())
    .await?;

    let row = row.ok_or(AppError::NotFound)?;
    Ok(Json(row.into()))
}

pub async fn get_skill_md(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug): Path<String>,
) -> AppResult<String> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let row: Option<(String,)> = sqlx::query_as(
        "SELECT bundle_uri FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(&slug)
    .fetch_optional(state.db())
    .await?;
    let (key,) = row.ok_or(AppError::NotFound)?;

    let bytes = state
        .storage()
        .read_bundle(&key)
        .await
        .map_err(AppError::Anyhow)?;

    let gz = GzDecoder::new(bytes.as_ref());
    let mut tar = tar::Archive::new(gz);
    for entry in tar
        .entries()
        .map_err(|e| AppError::BadRequest(e.to_string()))?
    {
        let mut entry = entry.map_err(|e| AppError::BadRequest(e.to_string()))?;
        let path = entry
            .path()
            .map_err(|e| AppError::BadRequest(e.to_string()))?
            .to_path_buf();
        if path.to_string_lossy().trim_start_matches("./") == "SKILL.md" {
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|e| AppError::BadRequest(e.to_string()))?;
            return Ok(buf);
        }
    }
    Err(AppError::NotFound)
}

pub async fn get_bundle(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug): Path<String>,
) -> AppResult<Response> {
    let row: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, bundle_uri FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(&slug)
    .fetch_optional(state.db())
    .await?;

    let (skill_id, key) = row.ok_or(AppError::NotFound)?;

    // Best-effort usage bump for the Phase 5 decay heuristic. Failure
    // here is logged but never blocks the response — a DB blip during
    // a download shouldn't fail the user's `skill-pool ensure`.
    bump_usage(state.db(), skill_id).await;

    if let Ok(Some(url)) = state.storage().presign_read(&key).await {
        return Ok(Redirect::temporary(&url).into_response());
    }

    let bytes = state
        .storage()
        .read_bundle(&key)
        .await
        .map_err(AppError::Anyhow)?;

    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/gzip"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{slug}.tar.gz\""))
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    Ok(resp)
}

/// Bump `use_count` + `last_used_at` for one skill. Best-effort: errors
/// are logged at warn level but never propagate.
async fn bump_usage(db: &sqlx::PgPool, skill_id: uuid::Uuid) {
    let r = sqlx::query(
        "UPDATE skills SET use_count = use_count + 1, last_used_at = now() WHERE id = $1",
    )
    .bind(skill_id)
    .execute(db)
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, skill_id = %skill_id, "bump_usage failed");
    }
}

#[derive(Deserialize)]
struct PublishMetadata {
    slug: String,
    version: String,
    #[serde(default)]
    when_to_use: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

pub async fn publish(
    State(state): State<AppState>,
    caller: AuthedCaller,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<Skill>)> {
    require_scope(&caller.scope, "skills:publish")?;

    let mut metadata_raw: Option<String> = None;
    let mut bundle_bytes: Option<Bytes> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        match field.name() {
            Some("metadata") => {
                metadata_raw = Some(
                    field
                        .text()
                        .await
                        .map_err(|e| AppError::BadRequest(format!("metadata: {e}")))?,
                );
            }
            Some("bundle") => {
                let bytes = field
                    .bytes()
                    .await
                    .map_err(|e| AppError::BadRequest(format!("bundle: {e}")))?;
                bundle_bytes = Some(bytes);
            }
            _ => continue,
        }
    }

    let metadata_raw =
        metadata_raw.ok_or_else(|| AppError::BadRequest("missing `metadata` field".into()))?;
    let bytes =
        bundle_bytes.ok_or_else(|| AppError::BadRequest("missing `bundle` field".into()))?;

    let meta: PublishMetadata = serde_json::from_str(&metadata_raw)
        .map_err(|e| AppError::BadRequest(format!("metadata JSON: {e}")))?;

    if meta.slug.is_empty() || meta.version.is_empty() {
        return Err(AppError::BadRequest("slug and version are required".into()));
    }

    let validated = bundle::validate(&bytes).map_err(bundle_to_app_err)?;

    // Persist to storage first; on DB conflict we leave one orphan blob — a
    // background sweeper (Phase 5) cleans those. The reverse order would
    // mean a successful DB row pointing at non-existent storage.
    let key = Storage::bundle_key(caller.tenant.tenant_id, &meta.slug, &meta.version);
    state
        .storage()
        .put_bundle(&key, bytes.clone())
        .await
        .map_err(AppError::Anyhow)?;

    let merged_tags: Vec<String> = {
        let mut t = validated.frontmatter.tags.clone();
        for tag in &meta.tags {
            if !t.contains(tag) {
                t.push(tag.clone());
            }
        }
        t
    };

    // Compute the description embedding so semantic search and dedup
    // can find this skill later. None when no embedder is configured.
    let embedding_literal = state
        .embedder()
        .embed(&validated.frontmatter.description)
        .map_err(AppError::Anyhow)?
        .map(|v| crate::embedding::vector_to_pg_literal(&v));

    let row: Result<SkillRow, sqlx::Error> = sqlx::query_as(
        "INSERT INTO skills \
           (tenant_id, slug, version, description, when_to_use, tags, bundle_uri, bundle_sha256, created_by, description_embedding) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9::text::vector) \
         RETURNING slug, version, description, when_to_use, tags, status, created_at",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&meta.slug)
    .bind(&meta.version)
    .bind(&validated.frontmatter.description)
    .bind(meta.when_to_use.as_ref().or(validated.frontmatter.when_to_use.as_ref()))
    .bind(&merged_tags)
    .bind(&key)
    .bind(&validated.sha256_hex)
    .bind(embedding_literal)
    .fetch_one(state.db())
    .await;

    let row = match row {
        Ok(r) => r,
        Err(sqlx::Error::Database(dbe))
            if dbe.constraint() == Some("skills_tenant_id_slug_version_key") =>
        {
            return Err(AppError::BadRequest(format!(
                "skill {}@{} already exists",
                meta.slug, meta.version
            )));
        }
        Err(e) => return Err(e.into()),
    };

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "skill.publish",
            target_kind: "skill",
            target_id: Some(&meta.slug),
            metadata: serde_json::json!({
                "version": meta.version,
                "size_bytes": validated.size_bytes,
                "sha256": validated.sha256_hex,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok((StatusCode::CREATED, Json(row.into())))
}

pub async fn validate(
    _state: State<AppState>,
    caller: AuthedCaller,
    mut multipart: Multipart,
) -> AppResult<Json<serde_json::Value>> {
    require_scope(&caller.scope, "skills:publish")?;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() == Some("bundle") {
            let bytes = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("bundle: {e}")))?;
            let v = bundle::validate(&bytes).map_err(bundle_to_app_err)?;
            return Ok(Json(serde_json::json!({
                "ok": true,
                "name": v.frontmatter.name,
                "description": v.frontmatter.description,
                "tags": v.frontmatter.tags,
                "size_bytes": v.size_bytes,
                "sha256": v.sha256_hex,
            })));
        }
    }
    Err(AppError::BadRequest("missing `bundle` field".into()))
}

// --- helpers --------------------------------------------------------------

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn bundle_to_app_err(e: BundleError) -> AppError {
    AppError::BadRequest(e.to_string())
}

#[derive(sqlx::FromRow)]
struct SkillRow {
    slug: String,
    version: String,
    description: String,
    when_to_use: Option<String>,
    tags: Vec<String>,
    status: String,
    created_at: DateTime<Utc>,
}

impl From<SkillRow> for Skill {
    fn from(r: SkillRow) -> Self {
        Self {
            slug: r.slug,
            version: r.version,
            description: r.description,
            when_to_use: r.when_to_use,
            tags: r.tags,
            status: r.status,
            created_at: r.created_at,
            similarity: None,
        }
    }
}

#[derive(sqlx::FromRow)]
struct SkillRowSemantic {
    slug: String,
    version: String,
    description: String,
    when_to_use: Option<String>,
    tags: Vec<String>,
    status: String,
    created_at: DateTime<Utc>,
    similarity: f32,
}

impl From<SkillRowSemantic> for Skill {
    fn from(r: SkillRowSemantic) -> Self {
        Self {
            slug: r.slug,
            version: r.version,
            description: r.description,
            when_to_use: r.when_to_use,
            tags: r.tags,
            status: r.status,
            created_at: r.created_at,
            similarity: Some(r.similarity),
        }
    }
}

// We also need to track caller user_id in the future; suppress unused warning for now.
#[allow(dead_code)]
fn _unused_metadata(_: &HashMap<String, String>) {}
