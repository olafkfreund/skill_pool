//! Skill drafts (Phase 4 — retrospective capture).
//!
//! Drafts are the inbox between "developer hit a wall and solved it" and
//! "team-published skill". A draft is a tar.gz bundle + metadata stored in
//! `skill_drafts`. A curator reviews via the web UI and either publishes
//! (promotes to `skills`) or discards (soft-marks the row).
//!
//! - `POST   /v1/drafts`              upload (multipart: bundle + metadata)
//! - `GET    /v1/drafts`              inbox list (?status=pending|all)
//! - `GET    /v1/drafts/{id}`         metadata for one
//! - `GET    /v1/drafts/{id}/skill-md`  rendered SKILL.md
//! - `POST   /v1/drafts/{id}/publish` promote to skills (body: { version, slug? })
//! - `POST   /v1/drafts/{id}/discard` mark discarded

use std::io::Read;

use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit;
use crate::auth::AuthedCaller;
use crate::bundle::{self, BundleError};
use crate::error::{AppError, AppResult};
use crate::git_sync;
use crate::state::AppState;
use crate::storage::Storage;

#[derive(Serialize, sqlx::FromRow)]
pub struct Draft {
    pub id: Uuid,
    pub slug: String,
    pub description: String,
    pub when_to_use: Option<String>,
    pub tags: Vec<String>,
    pub origin: String,
    pub notes: Option<String>,
    pub status: String,
    pub published_version: Option<String>,
    pub created_at: DateTime<Utc>,
    pub reviewed_at: Option<DateTime<Utc>>,
    /// When embedding-dedup flagged this draft as a near-duplicate of an
    /// existing skill, this is the target slug. NULL otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_proposal_slug: Option<String>,
    /// Cosine similarity to the proposed target. NULL when no proposal.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_proposal_similarity: Option<f32>,
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// `pending` (default), `published`, `discarded`, or `all`.
    pub status: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<Vec<Draft>>> {
    require_scope(&caller.scope, "skills:read")?;

    let status_filter = q.status.as_deref().unwrap_or("pending");
    // JUSTIFIED runtime-checked: two structurally-different queries chosen at runtime
    // based on whether `status_filter == "all"` (no status WHERE clause) vs. a
    // specific status value. The `query!` macro requires a single literal; branching
    // queries cannot be expressed as compile-time macros without duplicating the full
    // SELECT column list. Both queries are tenant-scoped via `d.tenant_id = $1`.
    let drafts: Vec<Draft> = if status_filter == "all" {
        sqlx::query_as(
            "SELECT d.id, d.slug, d.description, d.when_to_use, d.tags, d.origin, \
                    d.notes, d.status, d.published_version, d.created_at, d.reviewed_at, \
                    s.slug AS merge_proposal_slug, d.merge_proposal_similarity \
             FROM skill_drafts d \
             LEFT JOIN skills s ON s.id = d.merge_proposal_skill_id \
             WHERE d.tenant_id = $1 \
             ORDER BY d.status = 'pending' DESC, d.created_at DESC \
             LIMIT 200",
        )
        .bind(caller.tenant.tenant_id)
        .fetch_all(state.db())
        .await?
    } else {
        if !matches!(status_filter, "pending" | "published" | "discarded") {
            return Err(AppError::BadRequest(format!(
                "status must be one of: pending, published, discarded, all (got `{status_filter}`)"
            )));
        }
        sqlx::query_as(
            "SELECT d.id, d.slug, d.description, d.when_to_use, d.tags, d.origin, \
                    d.notes, d.status, d.published_version, d.created_at, d.reviewed_at, \
                    s.slug AS merge_proposal_slug, d.merge_proposal_similarity \
             FROM skill_drafts d \
             LEFT JOIN skills s ON s.id = d.merge_proposal_skill_id \
             WHERE d.tenant_id = $1 AND d.status = $2 \
             ORDER BY d.created_at DESC \
             LIMIT 200",
        )
        .bind(caller.tenant.tenant_id)
        .bind(status_filter)
        .fetch_all(state.db())
        .await?
    };

    Ok(Json(drafts))
}

pub async fn get_one(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Draft>> {
    require_scope(&caller.scope, "skills:read")?;
    let draft = load_draft(&state, caller.tenant.tenant_id, id).await?;
    Ok(Json(draft))
}

pub async fn get_skill_md(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<String> {
    require_scope(&caller.scope, "skills:read")?;

    let row = sqlx::query!(
        "SELECT bundle_uri FROM skill_drafts WHERE tenant_id = $1 AND id = $2",
        caller.tenant.tenant_id,
        id,
    )
    .fetch_optional(state.db())
    .await?;
    let key = row.ok_or(AppError::NotFound)?.bundle_uri;

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

#[derive(Deserialize)]
struct CreateMetadata {
    slug: String,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    when_to_use: Option<String>,
}

pub async fn create(
    State(state): State<AppState>,
    caller: AuthedCaller,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<Draft>)> {
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

    let meta: CreateMetadata = serde_json::from_str(&metadata_raw)
        .map_err(|e| AppError::BadRequest(format!("metadata JSON: {e}")))?;

    if meta.slug.trim().is_empty() {
        return Err(AppError::BadRequest("slug is required".into()));
    }

    let validated = bundle::validate(&bytes).map_err(bundle_to_app_err)?;
    let origin = meta.origin.as_deref().unwrap_or("cli");
    if !matches!(origin, "cli" | "capture-scorer" | "claude-hook" | "web") {
        return Err(AppError::BadRequest(format!(
            "origin must be one of: cli, capture-scorer, claude-hook, web (got `{origin}`)"
        )));
    }

    let merged_tags: Vec<String> = {
        let mut t = validated.frontmatter.tags.clone();
        for tag in &meta.tags {
            if !t.contains(tag) {
                t.push(tag.clone());
            }
        }
        t
    };

    let draft_id = Uuid::new_v4();
    let key = Storage::draft_bundle_key(caller.tenant.tenant_id, draft_id);

    state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?
        .put_bundle(&key, bytes.clone())
        .await
        .map_err(AppError::Anyhow)?;

    let when_to_use = meta
        .when_to_use
        .as_deref()
        .or(validated.frontmatter.when_to_use.as_deref());

    // Embedding-dedup (Phase 5). When no embedder is configured the
    // embedding stays NULL and the dedup query returns no row — schema
    // and code both degrade to the pre-Phase-5 behaviour.
    let embedding_vec = state
        .embedder()
        .embed(&validated.frontmatter.description)
        .map_err(AppError::Anyhow)?;
    let embedding_literal = embedding_vec
        .as_ref()
        .map(|v| crate::embedding::vector_to_pg_literal(v));

    // JUSTIFIED runtime-checked: `$2::text::vector` casts an embedding string
    // into a pgvector `vector` type. sqlx `query!` cannot express this cast
    // for a `String` argument without a native pgvector codec dependency.
    // Query is tenant-scoped via `tenant_id = $1`.
    let merge_proposal: Option<(Uuid, f32)> = if let Some(lit) = &embedding_literal {
        sqlx::query_as(
            "SELECT id, (1 - (description_embedding <=> $2::text::vector))::real AS similarity \
             FROM skills \
             WHERE tenant_id = $1 \
               AND status = 'published' \
               AND description_embedding IS NOT NULL \
             ORDER BY description_embedding <=> $2::text::vector ASC \
             LIMIT 1",
        )
        .bind(caller.tenant.tenant_id)
        .bind(lit)
        .fetch_optional(state.db())
        .await?
        .filter(|(_id, sim): &(Uuid, f32)| *sim >= crate::embedding::DEDUP_SIMILARITY_THRESHOLD)
    } else {
        None
    };

    // JUSTIFIED runtime-checked: `$12::text::vector` casts the embedding
    // string literal into pgvector's `vector` type. The macro cannot express
    // this cast for a nullable `Option<&str>` without a native pgvector codec.
    // Also contains a correlated subquery `(SELECT slug FROM skills WHERE id = $13)`
    // in RETURNING which `query!` cannot verify as a static literal.
    let row: Draft = sqlx::query_as(
        "INSERT INTO skill_drafts \
           (id, tenant_id, slug, description, when_to_use, tags, origin, notes, \
            bundle_uri, bundle_sha256, created_by, description_embedding, \
            merge_proposal_skill_id, merge_proposal_similarity) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12::text::vector, $13, $14) \
         RETURNING id, slug, description, when_to_use, tags, origin, notes, status, \
                   published_version, created_at, reviewed_at, \
                   (SELECT slug FROM skills WHERE id = $13) AS merge_proposal_slug, \
                   merge_proposal_similarity",
    )
    .bind(draft_id)
    .bind(caller.tenant.tenant_id)
    .bind(meta.slug.trim())
    .bind(&validated.frontmatter.description)
    .bind(when_to_use)
    .bind(&merged_tags)
    .bind(origin)
    .bind(meta.notes.as_deref())
    .bind(&key)
    .bind(&validated.sha256_hex)
    .bind(caller.user_id)
    .bind(embedding_literal.as_deref())
    .bind(merge_proposal.map(|(id, _)| id))
    .bind(merge_proposal.map(|(_, sim)| sim))
    .fetch_one(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "draft.create",
            target_kind: "skill_draft",
            target_id: Some(&draft_id.to_string()),
            metadata: serde_json::json!({
                "slug": meta.slug,
                "origin": origin,
                "size_bytes": validated.size_bytes,
                "sha256": validated.sha256_hex,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    // Curator webhook — fire-and-forget. Returns immediately; outcome is
    // audit-logged from the spawned task. Skipped silently if the tenant
    // has no webhook configured.
    crate::notify::draft_created(
        state.clone(),
        crate::notify::DraftCreatedEvent {
            tenant_id: caller.tenant.tenant_id,
            tenant_slug: caller.tenant.tenant_slug.clone(),
            draft_id,
            draft_slug: row.slug.clone(),
            description: row.description.clone(),
            origin: row.origin.clone(),
            merge_proposal_slug: row.merge_proposal_slug.clone(),
        },
    );

    Ok((StatusCode::CREATED, Json(row)))
}

#[derive(Deserialize)]
pub struct PublishBody {
    /// Required. Semver string assigned at publish time.
    pub version: String,
    /// Optional override; defaults to the draft's slug.
    #[serde(default)]
    pub slug: Option<String>,
}

#[derive(Serialize)]
pub struct PublishResponse {
    pub draft_id: Uuid,
    pub skill_id: Uuid,
    pub slug: String,
    pub version: String,
}

pub async fn publish(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
    Json(body): Json<PublishBody>,
) -> AppResult<Json<PublishResponse>> {
    require_scope(&caller.scope, "skills:publish")?;

    if body.version.trim().is_empty() {
        return Err(AppError::BadRequest("version is required".into()));
    }

    let mut tx = state.db().begin().await?;

    // Lock the draft row so two concurrent publishers don't double-promote.
    let draft = sqlx::query!(
        "SELECT id, slug, description, when_to_use, tags, bundle_uri, bundle_sha256, status \
         FROM skill_drafts \
         WHERE tenant_id = $1 AND id = $2 \
         FOR UPDATE",
        caller.tenant.tenant_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await?;

    let draft = draft.ok_or(AppError::NotFound)?;
    if draft.status != "pending" {
        return Err(AppError::BadRequest(format!(
            "draft is already {} — cannot publish",
            draft.status
        )));
    }

    let final_slug = body
        .slug
        .as_deref()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| draft.slug.clone());

    // Copy bundle to the canonical skill key. The opendal Operator doesn't
    // support cheap server-side copy on the FS backend, so read+write.
    // Same per-tenant storage for both read and write — a draft and its
    // promoted skill always live on the same backend.
    let storage = state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?;
    let bundle_bytes = storage
        .read_bundle(&draft.bundle_uri)
        .await
        .map_err(AppError::Anyhow)?;
    // Cheap clone (Bytes is ref-counted) — we need a copy for the
    // optional git-sync hook below; the original is moved into
    // `put_bundle`.
    let bundle_for_git = bundle_bytes.clone();
    let skill_key = Storage::bundle_key(caller.tenant.tenant_id, &final_slug, body.version.trim());
    storage
        .put_bundle(&skill_key, bundle_bytes)
        .await
        .map_err(AppError::Anyhow)?;

    let inserted: Result<Uuid, sqlx::Error> = sqlx::query_scalar!(
        "INSERT INTO skills \
           (tenant_id, slug, version, description, when_to_use, tags, bundle_uri, bundle_sha256, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
         RETURNING id",
        caller.tenant.tenant_id,
        final_slug,
        body.version.trim(),
        draft.description,
        draft.when_to_use,
        draft.tags.as_slice(),
        skill_key,
        draft.bundle_sha256,
        caller.user_id,
    )
    .fetch_one(&mut *tx)
    .await;

    let skill_id = match inserted {
        Ok(id) => id,
        Err(sqlx::Error::Database(dbe))
            if dbe.constraint() == Some("skills_tenant_id_slug_version_key") =>
        {
            return Err(AppError::BadRequest(format!(
                "{}@{} already exists — pick a different version",
                final_slug,
                body.version.trim()
            )));
        }
        Err(e) => return Err(e.into()),
    };

    sqlx::query!(
        "UPDATE skill_drafts \
         SET status = 'published', published_skill_id = $1, published_version = $2, \
             reviewed_by = $3, reviewed_at = now() \
         WHERE tenant_id = $4 AND id = $5",
        skill_id,
        body.version.trim(),
        caller.user_id,
        caller.tenant.tenant_id,
        id,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "draft.publish",
            target_kind: "skill_draft",
            target_id: Some(&id.to_string()),
            metadata: serde_json::json!({
                "slug": final_slug,
                "version": body.version,
                "skill_id": skill_id,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    // Best-effort Git mirror (#6 Phase 4). Detached; never blocks the
    // response. Disabled unless `SKILL_POOL_GIT_REPO_PATH` is set.
    // Drafts have no explicit `kind` column today — they all promote
    // to skills, so we hardcode `"skill"` here. Direct publishes via
    // `routes::skills::publish` use `meta.kind` and may write under
    // `agent/` or `command/` instead.
    if let Some(repo) = state.git_repo_path().map(std::path::Path::to_path_buf) {
        let tenant_slug = caller.tenant.tenant_slug.clone();
        let slug_for_git = final_slug.clone();
        let version_for_git = body.version.trim().to_string();
        // Re-extract SKILL.md from the bundle for the canonical write.
        // The bundle was just validated at create time, so this should
        // always succeed; on failure we log and skip git entirely.
        let skill_md = bundle::extract_skill_md(&bundle_for_git).ok();
        let bytes_clone = bundle_for_git;
        tokio::spawn(async move {
            let md = skill_md.unwrap_or_default();
            match git_sync::commit_skill(
                &repo,
                &tenant_slug,
                "skill",
                &slug_for_git,
                &version_for_git,
                &md,
                &bytes_clone,
            )
            .await
            {
                Ok(Some(sha)) => tracing::info!(
                    sha = %sha,
                    slug = %slug_for_git,
                    version = %version_for_git,
                    "git_sync: draft publish committed",
                ),
                Ok(None) => tracing::debug!("git_sync: skipped (disabled or best-effort fail)"),
                Err(e) => tracing::warn!(error = %e, "git_sync: commit_skill returned Err"),
            }
        });
    }

    Ok(Json(PublishResponse {
        draft_id: id,
        skill_id,
        slug: final_slug,
        version: body.version,
    }))
}

#[derive(Deserialize)]
pub struct PatchBody {
    /// Replace the slug. Trimmed and validated as non-empty.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    /// Empty string clears the column.
    #[serde(default)]
    pub when_to_use: Option<String>,
    /// Full replacement of the tag list. Pass `[]` to clear.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Empty string clears the reviewer note.
    #[serde(default)]
    pub notes: Option<String>,
}

/// Edit a pending draft's frontmatter metadata. The bundle body stays
/// read-only — if the curator needs to rewrite the body they should
/// discard and re-capture. Already-reviewed (published / discarded)
/// drafts are immutable.
pub async fn patch(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
    Json(body): Json<PatchBody>,
) -> AppResult<Json<Draft>> {
    require_scope(&caller.scope, "skills:publish")?;

    // Load + lock the row so we can validate state transitions before
    // building the dynamic UPDATE.
    let mut tx = state.db().begin().await?;
    let current = sqlx::query!(
        "SELECT id, slug, description, when_to_use, tags, bundle_uri, bundle_sha256, status \
         FROM skill_drafts \
         WHERE tenant_id = $1 AND id = $2 \
         FOR UPDATE",
        caller.tenant.tenant_id,
        id,
    )
    .fetch_optional(&mut *tx)
    .await?;
    let current = current.ok_or(AppError::NotFound)?;
    if current.status != "pending" {
        return Err(AppError::BadRequest(format!(
            "draft is {} — only pending drafts are editable",
            current.status
        )));
    }

    // Apply each field. Slug + description are non-empty when present;
    // empty strings on the nullable columns become NULL (clear).
    let slug = match body.slug.as_deref().map(str::trim) {
        Some("") => return Err(AppError::BadRequest("slug cannot be empty".into())),
        Some(s) => Some(s.to_string()),
        None => None,
    };
    let description = match body.description.as_deref().map(str::trim) {
        Some("") => {
            return Err(AppError::BadRequest("description cannot be empty".into()))
        }
        Some(s) => Some(s.to_string()),
        None => None,
    };
    // Capture the change flags before we consume the option values.
    let slug_changed = body.slug.is_some();
    let desc_changed = body.description.is_some();
    let when_changed = body.when_to_use.is_some();
    let tags_changed = body.tags.is_some();
    let notes_changed = body.notes.is_some();

    let when_to_use: Option<Option<String>> = body
        .when_to_use
        .map(|s| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) });
    let notes: Option<Option<String>> = body
        .notes
        .map(|s| if s.trim().is_empty() { None } else { Some(s.trim().to_string()) });

    // JUSTIFIED runtime-checked: `$5::int = 0` and `$8::int = 0` flag
    // parameters require explicit PostgreSQL casts that `query!` cannot
    // verify at compile time for integer flag arguments paired with nullable
    // text. The CASE … ELSE pattern is the canonical partial-update idiom
    // for nullable columns when three states must be distinguished (unset,
    // set-to-null, set-to-value). Tenant-scoped via `tenant_id = $1`.
    sqlx::query(
        "UPDATE skill_drafts SET \
            slug         = COALESCE($3, slug), \
            description  = COALESCE($4, description), \
            when_to_use  = CASE WHEN $5::int = 0 THEN when_to_use ELSE $6 END, \
            tags         = COALESCE($7, tags), \
            notes        = CASE WHEN $8::int = 0 THEN notes       ELSE $9 END \
         WHERE tenant_id = $1 AND id = $2",
    )
    .bind(caller.tenant.tenant_id)
    .bind(id)
    .bind(slug.as_deref())
    .bind(description.as_deref())
    .bind(if when_to_use.is_some() { 1_i32 } else { 0_i32 })
    .bind(when_to_use.unwrap_or(None))
    .bind(body.tags.as_ref())
    .bind(if notes.is_some() { 1_i32 } else { 0_i32 })
    .bind(notes.unwrap_or(None))
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    let fields_changed: Vec<&str> = [
        ("slug", slug_changed),
        ("description", desc_changed),
        ("when_to_use", when_changed),
        ("tags", tags_changed),
        ("notes", notes_changed),
    ]
    .into_iter()
    .filter_map(|(name, changed)| if changed { Some(name) } else { None })
    .collect();
    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "draft.update",
            target_kind: "skill_draft",
            target_id: Some(&id.to_string()),
            metadata: serde_json::json!({ "fields_changed": fields_changed }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(Json(load_draft(&state, caller.tenant.tenant_id, id).await?))
}

pub async fn discard(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "skills:publish")?;

    let result = sqlx::query!(
        "UPDATE skill_drafts \
         SET status = 'discarded', reviewed_by = $1, reviewed_at = now() \
         WHERE tenant_id = $2 AND id = $3 AND status = 'pending'",
        caller.user_id,
        caller.tenant.tenant_id,
        id,
    )
    .execute(state.db())
    .await?;

    if result.rows_affected() == 0 {
        // Either not found or not pending — either way, callers shouldn't
        // see a 200 they didn't earn.
        return Err(AppError::NotFound);
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "draft.discard",
            target_kind: "skill_draft",
            target_id: Some(&id.to_string()),
            metadata: serde_json::json!({}),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// --- helpers --------------------------------------------------------------

async fn load_draft(state: &AppState, tenant_id: Uuid, id: Uuid) -> AppResult<Draft> {
    let draft: Option<Draft> = sqlx::query_as(
        "SELECT d.id, d.slug, d.description, d.when_to_use, d.tags, d.origin, \
                d.notes, d.status, d.published_version, d.created_at, d.reviewed_at, \
                s.slug AS merge_proposal_slug, d.merge_proposal_similarity \
         FROM skill_drafts d \
         LEFT JOIN skills s ON s.id = d.merge_proposal_skill_id \
         WHERE d.tenant_id = $1 AND d.id = $2",
    )
    .bind(tenant_id)
    .bind(id)
    .fetch_optional(state.db())
    .await?;
    // JUSTIFIED runtime-checked: shares the same JOIN + alias shape as the
    // two `list()` queries above; uses `Draft` (sqlx::FromRow) which requires
    // runtime-checked `query_as` for the aliased merge_proposal_slug column.
    draft.ok_or(AppError::NotFound)
}

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

