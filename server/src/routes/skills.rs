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
    /// Catalog-item kind. Defaults to `skill`. Accepts `agent` or
    /// `command` as parallel surfaces (Phase 5+). Any other value is
    /// a 400.
    pub kind: Option<String>,
}

/// The three catalog-item kinds. Normalising the inbound string here
/// keeps all the SQL builders honest about valid values.
const VALID_KINDS: &[&str] = &["skill", "agent", "command"];
const DEFAULT_KIND: &str = "skill";

fn resolve_kind(raw: Option<&str>) -> AppResult<&'static str> {
    let v = raw.unwrap_or(DEFAULT_KIND).trim();
    match v {
        "skill" => Ok("skill"),
        "agent" => Ok("agent"),
        "command" => Ok("command"),
        other => Err(AppError::BadRequest(format!(
            "kind must be one of {:?}, got `{other}`",
            VALID_KINDS
        ))),
    }
}

pub async fn list(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<Skill>>> {
    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let kind = resolve_kind(q.kind.as_deref())?;
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
                 AND kind = $6 \
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
        .bind(kind)
        .fetch_all(state.db_read())
        .await?;

        return Ok(Json(rows.into_iter().map(Into::into).collect()));
    }

    // --- Keyword / tag / plain-list branch ------------------------------
    let needle = q.query.as_deref().map(|s| format!("%{s}%"));
    let rows: Vec<SkillRow> = sqlx::query_as(
        "SELECT DISTINCT ON (slug) \
            slug, version, description, when_to_use, tags, status, created_at \
         FROM skills \
         WHERE tenant_id = $1 \
           AND kind = $5 \
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
    .bind(kind)
    .fetch_all(state.db_read())
    .await?;

    Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// Tiny query struct shared by all get_* endpoints that take `:slug`.
/// `?kind=agent` etc. is how callers fetch a non-default kind by slug
/// until slice 2 adds dedicated `/v1/agents/...` paths.
#[derive(Deserialize)]
pub struct KindQuery {
    pub kind: Option<String>,
}

/// Query params for `GET /v1/skills/{slug}/bundle.tar.gz`.
///
/// `bytes=true` forces the proxy-bytes path even when the storage backend
/// supports presigned URLs. Two use cases:
///   - corporate proxies that strip cross-origin redirects
///   - test harnesses asserting on Content-Disposition headers
#[derive(Deserialize)]
pub struct BundleQuery {
    pub kind: Option<String>,
    #[serde(default)]
    pub bytes: bool,
}

pub async fn get_one(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug): Path<String>,
    Query(kq): Query<KindQuery>,
) -> AppResult<Json<Skill>> {
    let kind = resolve_kind(kq.kind.as_deref())?;
    let row: Option<SkillRow> = sqlx::query_as(
        "SELECT slug, version, description, when_to_use, tags, status, created_at \
         FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(&slug)
    .bind(kind)
    .fetch_optional(state.db_read())
    .await?;

    let row = row.ok_or(AppError::NotFound)?;
    Ok(Json(row.into()))
}

pub async fn get_skill_md(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Query(kq): Query<KindQuery>,
) -> AppResult<String> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let kind = resolve_kind(kq.kind.as_deref())?;
    let row: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, bundle_uri FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&slug)
    .bind(kind)
    .fetch_optional(state.db_read())
    .await?;
    let (skill_id, key) = row.ok_or(AppError::NotFound)?;

    // View event — same telemetry pipeline as download.
    record_usage(
        state.db(),
        caller.tenant.tenant_id,
        skill_id,
        "view",
        &caller,
    )
    .await;

    let bytes = state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?
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
    caller: AuthedCaller,
    Path(slug): Path<String>,
    Query(bq): Query<BundleQuery>,
) -> AppResult<Response> {
    let kind = resolve_kind(bq.kind.as_deref())?;
    let row: Option<(uuid::Uuid, String)> = sqlx::query_as(
        "SELECT id, bundle_uri FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&slug)
    .bind(kind)
    .fetch_optional(state.db_read())
    .await?;

    let (skill_id, key) = row.ok_or(AppError::NotFound)?;

    // Best-effort usage record: bumps the per-row counter for decay AND
    // appends an event row for the timeline aggregations. A DB blip here
    // is logged but never blocks the user's `skill-pool ensure`.
    record_usage(
        state.db(),
        caller.tenant.tenant_id,
        skill_id,
        "download",
        &caller,
    )
    .await;

    let storage = state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?;

    // Redirect to a short-lived signed URL when the backend supports it
    // (S3 / GCS / Azure) and the caller hasn't asked for bytes explicitly.
    // fs:// backends always fall through to streaming.
    if !bq.bytes {
        if let Ok(Some(url)) = storage.presign_read(&key).await {
            return Ok(Redirect::temporary(&url).into_response());
        }
    }

    let bytes = storage
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

/// Record one usage event: bumps the per-row counter on `skills` and
/// appends a row to `skill_usage_events` for the timeline aggregations.
/// Best-effort: errors are logged but never propagate.
async fn record_usage(
    db: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
    skill_id: uuid::Uuid,
    event_kind: &'static str,
    caller: &AuthedCaller,
) {
    let r = sqlx::query(
        "UPDATE skills SET use_count = use_count + 1, last_used_at = now() WHERE id = $1",
    )
    .bind(skill_id)
    .execute(db)
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, skill_id = %skill_id, "use_count bump failed");
    }

    let r = sqlx::query(
        "INSERT INTO skill_usage_events (tenant_id, skill_id, event_kind, user_id, token_id) \
         VALUES ($1, $2, $3, $4, $5)",
    )
    .bind(tenant_id)
    .bind(skill_id)
    .bind(event_kind)
    .bind(caller.user_id)
    .bind(caller.token_id)
    .execute(db)
    .await;
    if let Err(e) = r {
        tracing::warn!(error = ?e, skill_id = %skill_id, "usage event insert failed");
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
    /// Catalog kind. Defaults to `skill` for backward compat.
    #[serde(default)]
    kind: Option<String>,
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
    let kind = resolve_kind(meta.kind.as_deref())?;

    // Persist to storage first; on DB conflict we leave one orphan blob — a
    // background sweeper (Phase 5) cleans those. The reverse order would
    // mean a successful DB row pointing at non-existent storage.
    let key = Storage::bundle_key(caller.tenant.tenant_id, &meta.slug, &meta.version);
    state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?
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

    let row: Result<SkillRowWithId, sqlx::Error> = sqlx::query_as(
        "INSERT INTO skills \
           (tenant_id, slug, version, description, when_to_use, tags, bundle_uri, bundle_sha256, created_by, description_embedding, kind) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, $9::text::vector, $10) \
         RETURNING id, slug, version, description, when_to_use, tags, status, created_at",
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
    .bind(kind)
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

    // Phase 5 — dependency declarations. Each `requires` entry in the
    // SKILL.md frontmatter becomes one row in skill_dependencies.
    // Forward references are fine: the target slug doesn't need to exist
    // yet; the closure endpoint resolves at read time.
    //
    // Conflict detection (#7): before inserting, check whether any OTHER
    // published skill in this tenant requires the same slug at an
    // incompatible version range. If so the publish fails with 409 and
    // names both skills + ranges so the operator can resolve.
    for req in &validated.frontmatter.requires {
        let (req_slug, version_range) = parse_requires_entry(req);
        if req_slug.is_empty() {
            return Err(AppError::BadRequest(format!(
                "invalid requires entry `{req}` (expected `slug` or `slug@version`)"
            )));
        }
        if req_slug == meta.slug {
            return Err(AppError::BadRequest(format!(
                "skill `{req_slug}` cannot require itself"
            )));
        }
        check_version_compatibility(
            state.db(),
            caller.tenant.tenant_id,
            &meta.slug,
            &req_slug,
            &version_range,
        )
        .await?;
        sqlx::query(
            "INSERT INTO skill_dependencies \
               (tenant_id, parent_skill_id, requires_slug, version_range) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (parent_skill_id, requires_slug) DO UPDATE SET version_range = EXCLUDED.version_range",
        )
        .bind(caller.tenant.tenant_id)
        .bind(row.id)
        .bind(&req_slug)
        .bind(&version_range)
        .execute(state.db())
        .await?;
    }

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
                "requires": validated.frontmatter.requires.len(),
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok((StatusCode::CREATED, Json(row.into())))
}

/// One entry in a skill's dependency closure.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DepEntry {
    pub slug: String,
    pub version_range: String,
    pub depth: i32,
}

/// Reverse-edge entry: a skill that requires *us*.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct DependentEntry {
    pub slug: String,
    pub version: String,
    pub version_range: String,
}

/// Pending-draft entry that flagged this skill as its merge target.
#[derive(serde::Serialize, sqlx::FromRow)]
pub struct PendingMergeProposal {
    pub draft_id: uuid::Uuid,
    pub draft_slug: String,
    pub similarity: f32,
}

/// Detail view for the portal's per-skill page. Bundles base metadata +
/// usage counters + forward deps + reverse deps + pending merge proposals
/// in one round-trip so the SvelteKit loader stays a single `await`.
#[derive(serde::Serialize)]
pub struct SkillDetail {
    pub slug: String,
    pub version: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub tags: Vec<String>,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub use_count: i32,
    pub last_used_at: Option<DateTime<Utc>>,
    pub requires: Vec<DependentEntry>,
    pub required_by: Vec<DependentEntry>,
    pub merge_proposals: Vec<PendingMergeProposal>,
}

#[derive(sqlx::FromRow)]
struct SkillDetailRow {
    id: uuid::Uuid,
    slug: String,
    version: String,
    description: String,
    when_to_use: Option<String>,
    tags: Vec<String>,
    status: String,
    created_at: DateTime<Utc>,
    use_count: i32,
    last_used_at: Option<DateTime<Utc>>,
}

pub async fn get_detail(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug): Path<String>,
    Query(kq): Query<KindQuery>,
) -> AppResult<Json<SkillDetail>> {
    let kind = resolve_kind(kq.kind.as_deref())?;
    let parent: Option<SkillDetailRow> = sqlx::query_as(
        "SELECT id, slug, version, description, when_to_use, tags, status, created_at, \
                use_count, last_used_at \
         FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(&slug)
    .bind(kind)
    .fetch_optional(state.db_read())
    .await?;
    let parent = parent.ok_or(AppError::NotFound)?;

    // Forward edges: rows in skill_dependencies whose parent is this
    // skill, joined to skills to surface the target's current version
    // (if published). Unpublished target → version is empty string.
    let requires: Vec<DependentEntry> = sqlx::query_as(
        "SELECT d.requires_slug AS slug, \
                COALESCE(s.version, '') AS version, \
                d.version_range \
         FROM skill_dependencies d \
         LEFT JOIN LATERAL ( \
            SELECT version FROM skills \
            WHERE tenant_id = $1 AND slug = d.requires_slug AND status = 'published' \
            ORDER BY created_at DESC LIMIT 1 \
         ) s ON true \
         WHERE d.tenant_id = $1 AND d.parent_skill_id = $2 \
         ORDER BY d.requires_slug ASC",
    )
    .bind(tenant.tenant_id)
    .bind(parent.id)
    .fetch_all(state.db_read())
    .await?;

    // Reverse edges: who declares a dependency on this slug?
    let required_by: Vec<DependentEntry> = sqlx::query_as(
        "SELECT s.slug AS slug, s.version AS version, d.version_range \
         FROM skill_dependencies d \
         JOIN skills s ON s.id = d.parent_skill_id AND s.status = 'published' \
         WHERE d.tenant_id = $1 AND d.requires_slug = $2 \
         ORDER BY s.slug ASC",
    )
    .bind(tenant.tenant_id)
    .bind(&parent.slug)
    .fetch_all(state.db_read())
    .await?;

    // Pending drafts that flagged this slug as a merge target (Phase 5
    // embedding dedup). Joining through skills.slug rather than the row
    // id surfaces proposals against any version of this skill — the
    // semantic intent is "are curators proposing a merge here?", not
    // "which row id was the embedding closest to?".
    let merge_proposals: Vec<PendingMergeProposal> = sqlx::query_as(
        "SELECT d.id AS draft_id, d.slug AS draft_slug, \
                d.merge_proposal_similarity AS similarity \
         FROM skill_drafts d \
         JOIN skills s ON s.id = d.merge_proposal_skill_id AND s.tenant_id = $1 \
         WHERE d.tenant_id = $1 \
           AND d.status = 'pending' \
           AND s.slug = $2 \
         ORDER BY d.merge_proposal_similarity DESC NULLS LAST \
         LIMIT 10",
    )
    .bind(tenant.tenant_id)
    .bind(&parent.slug)
    .fetch_all(state.db_read())
    .await?;

    Ok(Json(SkillDetail {
        slug: parent.slug,
        version: parent.version,
        description: parent.description,
        when_to_use: parent.when_to_use,
        tags: parent.tags,
        status: parent.status,
        created_at: parent.created_at,
        use_count: parent.use_count,
        last_used_at: parent.last_used_at,
        requires,
        required_by,
        merge_proposals,
    }))
}

/// `GET /v1/skills/{slug}/deps` — return the transitive dependency
/// closure of a published skill. Cycle-safe (UNION dedups; depth cap
/// is belt-and-braces). Tenant-scoped.
pub async fn get_deps(
    State(state): State<AppState>,
    tenant: TenantCtx,
    Path(slug): Path<String>,
) -> AppResult<Json<Vec<DepEntry>>> {
    // Resolve the latest published version of `slug` for this tenant.
    let parent: Option<(uuid::Uuid,)> = sqlx::query_as(
        "SELECT id FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(&slug)
    .fetch_optional(state.db_read())
    .await?;
    let Some((parent_id,)) = parent else {
        return Err(AppError::NotFound);
    };

    let rows: Vec<DepEntry> = sqlx::query_as(
        "WITH RECURSIVE closure AS ( \
            SELECT d.requires_slug, d.version_range, 1 AS depth \
            FROM skill_dependencies d \
            WHERE d.tenant_id = $1 AND d.parent_skill_id = $2 \
            \
            UNION \
            \
            SELECT d.requires_slug, d.version_range, c.depth + 1 \
            FROM skill_dependencies d \
            JOIN skills s ON s.id = d.parent_skill_id AND s.tenant_id = $1 \
            JOIN closure c ON s.slug = c.requires_slug \
            WHERE c.depth < 10 \
         ) \
         SELECT requires_slug AS slug, version_range, depth \
         FROM closure \
         ORDER BY depth ASC, slug ASC",
    )
    .bind(tenant.tenant_id)
    .bind(parent_id)
    .fetch_all(state.db_read())
    .await?;

    Ok(Json(rows))
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
struct SkillRowWithId {
    id: uuid::Uuid,
    slug: String,
    version: String,
    description: String,
    when_to_use: Option<String>,
    tags: Vec<String>,
    status: String,
    created_at: DateTime<Utc>,
}

impl From<SkillRowWithId> for Skill {
    fn from(r: SkillRowWithId) -> Self {
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

/// Reject the publish when another published skill in this tenant
/// already requires `req_slug` at an incompatible `version_range`.
///
/// v1 semantics (intentionally narrow, see `docs/lifecycle.md` "Future
/// work"):
///   - `*` matches anything → never conflicts.
///   - Two non-`*` ranges are compatible iff they are byte-identical.
///   - More complex ranges (`^1.2`, `>=1.0,<2.0`) are treated as opaque
///     strings — they conflict with any other non-equal range. The
///     tradeoff: false-positives push operators to align ranges
///     explicitly, which is the right outcome until we ship a semver
///     resolver.
async fn check_version_compatibility(
    db: &sqlx::PgPool,
    tenant_id: uuid::Uuid,
    new_slug: &str,
    req_slug: &str,
    new_range: &str,
) -> AppResult<()> {
    // `*` never conflicts with anything.
    if new_range == "*" {
        return Ok(());
    }
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT s.slug, sd.version_range \
         FROM skill_dependencies sd \
         JOIN skills s ON s.id = sd.parent_skill_id \
         WHERE sd.tenant_id = $1 \
           AND sd.requires_slug = $2 \
           AND s.slug <> $3 \
           AND s.status = 'published'",
    )
    .bind(tenant_id)
    .bind(req_slug)
    .bind(new_slug)
    .fetch_all(db)
    .await?;

    for (other_slug, other_range) in rows {
        if !ranges_compatible(new_range, &other_range) {
            return Err(AppError::Conflict(format!(
                "skill `{new_slug}` requires `{req_slug}@{new_range}` but skill `{other_slug}` already requires `{req_slug}@{other_range}`"
            )));
        }
    }
    Ok(())
}

/// v1 compatibility predicate. See `check_version_compatibility` docs.
fn ranges_compatible(a: &str, b: &str) -> bool {
    a == "*" || b == "*" || a == b
}

/// Parse a `requires` entry. Accepts either `slug` or `slug@version`.
/// Empty slug is returned when the input is malformed; caller surfaces
/// that as a 400.
fn parse_requires_entry(raw: &str) -> (String, String) {
    let trimmed = raw.trim();
    match trimmed.split_once('@') {
        Some((slug, version)) => {
            let v = version.trim();
            (
                slug.trim().to_string(),
                if v.is_empty() { "*".into() } else { v.to_string() },
            )
        }
        None => (trimmed.to_string(), "*".into()),
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
