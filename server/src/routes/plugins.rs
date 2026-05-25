//! Plugins (Layer 3) — REST surface mirroring `/v1/skills`.
//!
//! Route table:
//!
//! - `POST   /v1/plugins`                                     publish
//! - `GET    /v1/plugins`                                     list (paginated)
//! - `GET    /v1/plugins/{slug}`                              latest published version
//! - `GET    /v1/plugins/{slug}/versions`                     version history
//! - `DELETE /v1/plugins/{slug}/versions/{version}`           archive (soft-delete)
//!
//! Scope policy (mirrors `routes/skills.rs` + `routes/projects.rs`):
//!   - writes (`publish`, `archive`)  → caller must carry `skills:publish` (or `*`).
//!     Both the `curator` and `admin` roles in `auth::role_to_scope` already grant
//!     this; intentionally do NOT invent a `tenant:curator` scope string.
//!   - reads (`list`, `get_one`, `get_versions`) → any authenticated tenant member.
//!
//! Tenant scoping is handled by `AuthedCaller`, which embeds a validated
//! `TenantCtx`. Every query filters on `tenant_id = $1` so cross-tenant
//! reads are impossible by construction.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum size of the serialised `manifest` JSON.
///
/// 256 KiB matches the issue's stated cap and stays well below the
/// per-request body limit applied by the router. The check runs against
/// the JSON re-serialised by `serde_json::to_vec` so we measure the
/// canonical byte length, not the inbound encoding.
const MAX_MANIFEST_BYTES: usize = 256 * 1024;

const DEFAULT_LIMIT: i64 = 50;
const MAX_LIMIT: i64 = 200;

const VALID_CONTENT_KINDS: &[&str] = &["skill", "agent", "command"];
const VALID_SOURCING_MODES: &[&str] = &["internal", "external", "mirror"];
const VALID_STATUSES: &[&str] = &["draft", "published", "archived"];

// ---------------------------------------------------------------------------
// Request bodies
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct PublishBody {
    /// Plugin slug — the registry-side identifier used in URLs. Distinct
    /// from `manifest.name`, which is the human-facing display name.
    pub slug: String,

    /// Raw `.claude-plugin/plugin.json` body. Validated below + stored
    /// verbatim as JSONB in `plugins.manifest`.
    pub manifest: serde_json::Value,

    /// Skill/agent/command items bundled by this plugin. Each entry must
    /// reference a row that is currently `status='published'` in this
    /// tenant; cross-tenant references are rejected by the publish-time
    /// lookup (tenant-scoped) and additionally by the composite FK from
    /// `plugin_marketplace_entries` (defense in depth, schema-layer).
    pub contents: Vec<PluginContentInput>,

    /// One of `internal | external | mirror`. The schema `CHECK` enforces
    /// the same set; we validate at the API layer for a clean 400.
    pub sourcing_mode: String,

    #[serde(default)]
    pub external_git_url: Option<String>,

    #[serde(default)]
    pub upstream_url: Option<String>,

    /// `draft` or `published` (default `published`). `archived` is not a
    /// valid initial state — use the DELETE endpoint to archive.
    #[serde(default)]
    pub status: Option<String>,
}

#[derive(Deserialize)]
pub struct PluginContentInput {
    pub kind: String,
    pub slug: String,
    pub version: String,
}

#[derive(Deserialize)]
pub struct ListQuery {
    /// Comma-separated tag list. ALL must match (mirrors `/v1/skills`).
    /// Tags are read from `manifest -> 'tags'`; absent → never matches a
    /// tag filter.
    pub tags: Option<String>,
    /// Defaults to `published` so the catalog view only shows live items.
    pub status: Option<String>,
    pub sourcing_mode: Option<String>,
    pub limit: Option<i64>,
    /// Opaque cursor returned by the previous response in `next_cursor`.
    /// Encodes `(created_at, id)` so pagination is stable under inserts.
    pub cursor: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct PluginResponse {
    pub slug: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub sourcing_mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_git_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream_url: Option<String>,
    pub manifest: serde_json::Value,
    pub contents: Vec<PluginContentResponse>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct PluginContentResponse {
    pub kind: String,
    pub slug: String,
    pub version: String,
    pub position: i32,
}

#[derive(Serialize)]
pub struct PluginListResponse {
    pub items: Vec<PluginListRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize)]
pub struct PluginListRow {
    pub slug: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub status: String,
    pub sourcing_mode: String,
    pub tags: Vec<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Serialize)]
pub struct PluginVersionRow {
    pub version: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_by: Option<String>,
}

// ---------------------------------------------------------------------------
// Scope helpers (mirror routes/skills.rs + routes/projects.rs)
// ---------------------------------------------------------------------------

fn require_publish(scope: &str) -> AppResult<()> {
    if scope
        .split_whitespace()
        .any(|s| s == "skills:publish" || s == "*")
    {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn require_read(scope: &str) -> AppResult<()> {
    // Any authenticated caller is a tenant member — mirrors
    // `routes::projects::require_member`.
    if scope.is_empty() {
        Err(AppError::Forbidden)
    } else {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// `POST /v1/plugins` — publish a new plugin version.
///
/// Status mapping for validation failures (in check order):
///   - body manifest > 256 KiB              → 413
///   - manifest missing required fields      → 422 (field map)
///   - sourcing_mode invalid                 → 400
///   - paired URL missing for ext/mirror     → 422 (field map)
///   - `contents[i].kind` invalid            → 422 (field map)
///   - any content slug+version not published in this tenant → 422 (field map)
///   - `(tenant, slug, version)` already exists → 409
pub async fn publish(
    State(state): State<AppState>,
    caller: AuthedCaller,
    headers: HeaderMap,
    Json(body): Json<PublishBody>,
) -> AppResult<(StatusCode, Json<PluginResponse>)> {
    require_publish(&caller.scope)?;

    // 0. Slug — registry identifier (not the same as manifest.name).
    let slug = body.slug.trim();
    if slug.is_empty() {
        return Err(AppError::BadRequest("slug is required".into()));
    }

    // 1. Size guard. We measure the canonical re-serialisation so the
    //    cap holds regardless of inbound whitespace.
    let manifest_bytes = serde_json::to_vec(&body.manifest)
        .map_err(|e| AppError::BadRequest(format!("manifest serialisation: {e}")))?;
    if manifest_bytes.len() > MAX_MANIFEST_BYTES {
        return Err(AppError::PayloadTooLarge(format!(
            "manifest is {} bytes; limit is {}",
            manifest_bytes.len(),
            MAX_MANIFEST_BYTES
        )));
    }

    // 2. Structural manifest validation. Builds up a field-keyed error
    //    map and emits one 422 covering every problem found, so the
    //    client renders all field errors at once instead of fix-one-
    //    retry-find-next.
    let mut errs = serde_json::Map::<String, serde_json::Value>::new();
    let manifest_obj = body.manifest.as_object().ok_or_else(|| {
        let mut m = serde_json::Map::new();
        m.insert(
            "manifest".into(),
            serde_json::Value::String("must be a JSON object".into()),
        );
        AppError::Unprocessable(serde_json::Value::Object(m))
    })?;

    let manifest_name = string_field(manifest_obj, "name");
    let manifest_version = string_field(manifest_obj, "version");
    let manifest_description = string_field(manifest_obj, "description");

    if manifest_name.as_deref().map(str::is_empty).unwrap_or(true) {
        errs.insert(
            "name".into(),
            serde_json::Value::String("required and non-empty".into()),
        );
    }
    if manifest_version
        .as_deref()
        .map(str::is_empty)
        .unwrap_or(true)
    {
        errs.insert(
            "version".into(),
            serde_json::Value::String("required and non-empty".into()),
        );
    }
    if manifest_description
        .as_deref()
        .map(str::is_empty)
        .unwrap_or(true)
    {
        errs.insert(
            "description".into(),
            serde_json::Value::String("required and non-empty".into()),
        );
    }

    // 3. Sourcing mode — 400 (invalid enum), URL pairing → 422.
    if !VALID_SOURCING_MODES.contains(&body.sourcing_mode.as_str()) {
        return Err(AppError::BadRequest(format!(
            "sourcing_mode must be one of {:?}, got `{}`",
            VALID_SOURCING_MODES, body.sourcing_mode
        )));
    }
    let ext = body
        .external_git_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let up = body
        .upstream_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if body.sourcing_mode == "external" && ext.is_none() {
        errs.insert(
            "external_git_url".into(),
            serde_json::Value::String("required when sourcing_mode=external".into()),
        );
    }
    if body.sourcing_mode == "mirror" && up.is_none() {
        errs.insert(
            "upstream_url".into(),
            serde_json::Value::String("required when sourcing_mode=mirror".into()),
        );
    }

    // 4. Content kinds — bad enum is per-index 422.
    for (i, c) in body.contents.iter().enumerate() {
        if !VALID_CONTENT_KINDS.contains(&c.kind.as_str()) {
            errs.insert(
                format!("contents[{i}].kind"),
                serde_json::Value::String(format!(
                    "must be one of {:?}, got `{}`",
                    VALID_CONTENT_KINDS, c.kind
                )),
            );
        }
    }

    if !errs.is_empty() {
        return Err(AppError::Unprocessable(serde_json::Value::Object(errs)));
    }

    // 5. Status — defaults to `published` (caller can publish straight
    //    through). `archived` is not a valid initial state.
    let target_status = body.status.as_deref().unwrap_or("published").to_string();
    if !VALID_STATUSES.contains(&target_status.as_str()) || target_status == "archived" {
        return Err(AppError::BadRequest(format!(
            "status must be one of {:?} (not `archived`), got `{}`",
            ["draft", "published"],
            target_status
        )));
    }

    // 6. Content reference validation — every (slug, kind, version) must
    //    map to a published row in THIS tenant. One round-trip via array
    //    parameters so cost scales with N contents, not N queries.
    if !body.contents.is_empty() {
        let slugs: Vec<&str> = body.contents.iter().map(|c| c.slug.as_str()).collect();
        let kinds: Vec<&str> = body.contents.iter().map(|c| c.kind.as_str()).collect();
        let versions: Vec<&str> = body.contents.iter().map(|c| c.version.as_str()).collect();
        // sqlx unnest pattern: parallel arrays joined into a row source.
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT s.slug, s.kind, s.version \
             FROM skills s \
             JOIN unnest($2::text[], $3::text[], $4::text[]) \
               AS req(slug, kind, version) \
               ON s.slug = req.slug AND s.kind = req.kind AND s.version = req.version \
             WHERE s.tenant_id = $1 AND s.status = 'published'",
        )
        .bind(caller.tenant.tenant_id)
        .bind(&slugs)
        .bind(&kinds)
        .bind(&versions)
        .fetch_all(state.db_read())
        .await?;

        let found: std::collections::HashSet<(String, String, String)> = rows.into_iter().collect();
        let mut missing = serde_json::Map::<String, serde_json::Value>::new();
        for (i, c) in body.contents.iter().enumerate() {
            let key = (c.slug.clone(), c.kind.clone(), c.version.clone());
            if !found.contains(&key) {
                missing.insert(
                    format!("contents[{i}]"),
                    serde_json::Value::String(format!(
                        "{}@{} (kind={}) is not published in this tenant",
                        c.slug, c.version, c.kind
                    )),
                );
            }
        }
        if !missing.is_empty() {
            return Err(AppError::Unprocessable(serde_json::Value::Object(missing)));
        }
    }

    // 7. Insert the plugin row inside a transaction so that a duplicate
    //    `(slug, version)` rolls back the contents we'd otherwise leak.
    let mut tx = state.db().begin().await?;

    let name = manifest_name.unwrap();
    let version = manifest_version.unwrap();
    let description = manifest_description; // already validated non-empty above

    let insert_res = sqlx::query!(
        "INSERT INTO plugins \
           (tenant_id, slug, version, name, description, manifest, status, \
            sourcing_mode, external_git_url, upstream_url, created_by) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11) \
         RETURNING id, slug::text AS \"slug!\", version, name, description, manifest, \
                   status, sourcing_mode, external_git_url, upstream_url, \
                   created_at, updated_at",
        caller.tenant.tenant_id,
        slug,
        version,
        name,
        description,
        body.manifest,
        target_status,
        body.sourcing_mode,
        ext.as_deref(),
        up.as_deref(),
        caller.user_id,
    )
    .fetch_one(&mut *tx)
    .await;

    let row = match insert_res {
        Ok(r) => r,
        Err(sqlx::Error::Database(dbe))
            if dbe.constraint() == Some("plugins_tenant_id_slug_version_key") =>
        {
            return Err(AppError::Conflict(format!(
                "plugin {slug}@{version} already exists"
            )));
        }
        Err(e) => return Err(e.into()),
    };

    // Insert contents preserving caller order via `position`.
    for (position, c) in body.contents.iter().enumerate() {
        sqlx::query!(
            "INSERT INTO plugin_contents \
               (plugin_id, content_slug, content_kind, content_version, position) \
             VALUES ($1, $2, $3, $4, $5)",
            row.id,
            c.slug,
            c.kind,
            c.version,
            position as i32,
        )
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    // Marketplace materialisation hook (#31). Two best-effort side effects
    // that run after the publish DB row is durable:
    //
    //   1. Internal plugins: write the canonical filesystem layout into a
    //      bare git repo so Claude Code's `/plugin install` can clone us.
    //   2. Upsert plugin_marketplace_entries so the next
    //      `.claude-plugin/marketplace.json` fetch surfaces this plugin.
    //
    // Both are best-effort: a transient storage hiccup logs a warning but
    // does not roll back the API publish. The plugin row still exists; an
    // admin can republish to retry. A successful publish without a
    // marketplace entry simply doesn't appear in marketplace.json yet —
    // safer than failing the whole publish and surfacing an inconsistent
    // mid-state to the caller.
    if target_status == "published" {
        if body.sourcing_mode == "internal" {
            let content_refs: Vec<crate::plugin_git::ContentRef> = body
                .contents
                .iter()
                .map(|c| crate::plugin_git::ContentRef {
                    kind: c.kind.clone(),
                    slug: c.slug.clone(),
                    version: c.version.clone(),
                })
                .collect();
            if let Err(e) = crate::plugin_git::materialise_internal(
                &state,
                &caller.tenant,
                slug,
                &version,
                &row.manifest,
                &content_refs,
            )
            .await
            {
                tracing::warn!(
                    error = ?e,
                    tenant = %caller.tenant.tenant_slug,
                    slug = %slug,
                    version = %version,
                    "plugin git materialisation failed; marketplace entry skipped",
                );
            } else if let Err(e) = crate::routes::marketplace::regenerate_entry(
                &state,
                &caller.tenant,
                slug,
                &version,
                row.id,
                &body.sourcing_mode,
                ext.as_deref(),
                &row.manifest,
                &crate::routes::marketplace::origin_from_request(&headers),
            )
            .await
            {
                tracing::warn!(
                    error = ?e,
                    tenant = %caller.tenant.tenant_slug,
                    slug = %slug,
                    "marketplace entry upsert failed",
                );
            }
        } else {
            // External/mirror: no local git materialisation, just the
            // marketplace entry. (Mirror plugins' git tree is written by
            // the mirror worker landing in #36 follow-up; the entry's
            // source URL still points at our git endpoint via the
            // `mirror` branch in build_source.)
            if let Err(e) = crate::routes::marketplace::regenerate_entry(
                &state,
                &caller.tenant,
                slug,
                &version,
                row.id,
                &body.sourcing_mode,
                ext.as_deref(),
                &row.manifest,
                &crate::routes::marketplace::origin_from_request(&headers),
            )
            .await
            {
                tracing::warn!(
                    error = ?e,
                    tenant = %caller.tenant.tenant_slug,
                    slug = %slug,
                    "marketplace entry upsert failed (external/mirror)",
                );
            }
        }
    }

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "plugin.publish",
            target_kind: "plugin",
            target_id: Some(slug),
            metadata: serde_json::json!({
                "slug": slug,
                "version": version,
                "sourcing_mode": body.sourcing_mode,
                "content_count": body.contents.len(),
                "manifest_bytes": manifest_bytes.len(),
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    let contents = body
        .contents
        .into_iter()
        .enumerate()
        .map(|(position, c)| PluginContentResponse {
            kind: c.kind,
            slug: c.slug,
            version: c.version,
            position: position as i32,
        })
        .collect();

    Ok((
        StatusCode::CREATED,
        Json(PluginResponse {
            slug: row.slug,
            version: row.version,
            name: row.name,
            description: row.description,
            status: row.status,
            sourcing_mode: row.sourcing_mode,
            external_git_url: row.external_git_url,
            upstream_url: row.upstream_url,
            manifest: row.manifest,
            contents,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }),
    ))
}

/// `GET /v1/plugins?tags=&status=&sourcing_mode=&limit=&cursor=` — paginated list.
pub async fn list(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Query(q): Query<ListQuery>,
) -> AppResult<Json<PluginListResponse>> {
    require_read(&caller.scope)?;

    let limit = q.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let status = q.status.as_deref().unwrap_or("published");
    if !VALID_STATUSES.contains(&status) {
        return Err(AppError::BadRequest(format!(
            "status must be one of {:?}, got `{status}`",
            VALID_STATUSES
        )));
    }
    let sourcing_filter = match q.sourcing_mode.as_deref() {
        None => None,
        Some(s) if VALID_SOURCING_MODES.contains(&s) => Some(s.to_string()),
        Some(s) => {
            return Err(AppError::BadRequest(format!(
                "sourcing_mode must be one of {:?}, got `{s}`",
                VALID_SOURCING_MODES
            )));
        }
    };
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

    let (cursor_ts, cursor_id) = match q.cursor.as_deref() {
        None => (None, None),
        Some(c) => {
            let (ts, id) = decode_cursor(c)?;
            (Some(ts), Some(id))
        }
    };

    // DISTINCT ON keeps the latest-version-per-slug semantics consistent
    // with `/v1/skills`. The keyset cursor compares against the
    // discriminating `(created_at, id)` so duplicates and concurrent
    // inserts can't cause a row to be skipped or repeated.
    //
    // Tag filtering pulls `manifest->'tags'` (when present) and applies
    // `@>` containment, mirroring the `tags @>` test on `/v1/skills`.
    // Plugins whose manifest has no `tags` array never match a tag filter
    // — same as a skill without tags.
    let rows = sqlx::query!(
        "WITH latest AS ( \
           SELECT DISTINCT ON (slug) \
             id, slug::text AS slug, version, name, description, status, \
             sourcing_mode, manifest, created_at \
           FROM plugins \
           WHERE tenant_id = $1 \
             AND status = $2 \
             AND ($3::text IS NULL OR sourcing_mode = $3) \
           ORDER BY slug, created_at DESC \
         ) \
         SELECT id, slug, version, name, description, status, sourcing_mode, \
                manifest, created_at \
         FROM latest \
         WHERE ($4::text[] = '{}' OR ( \
                 CASE WHEN jsonb_typeof(manifest->'tags') = 'array' \
                      THEN ARRAY(SELECT jsonb_array_elements_text(manifest->'tags')) \
                      ELSE ARRAY[]::text[] END \
               ) @> $4) \
           AND ($5::timestamptz IS NULL OR (created_at, id) < ($5, $6)) \
         ORDER BY created_at DESC, id DESC \
         LIMIT $7",
        caller.tenant.tenant_id,
        status,
        sourcing_filter.as_deref(),
        &tag_list,
        cursor_ts,
        cursor_id,
        limit,
    )
    .fetch_all(state.db_read())
    .await?;

    let mut items: Vec<PluginListRow> = Vec::with_capacity(rows.len());
    let mut last: Option<(DateTime<Utc>, uuid::Uuid)> = None;
    for r in &rows {
        last = Some((r.created_at, r.id));
        items.push(PluginListRow {
            slug: r.slug.clone().unwrap_or_default(),
            version: r.version.clone(),
            name: r.name.clone(),
            description: r.description.clone(),
            status: r.status.clone(),
            sourcing_mode: r.sourcing_mode.clone(),
            tags: extract_tags(&r.manifest),
            created_at: r.created_at,
        });
    }

    // Only surface a cursor when we filled the page; a short page is
    // the natural EOF signal.
    let next_cursor = if rows.len() as i64 == limit {
        last.map(|(ts, id)| encode_cursor(ts, id))
    } else {
        None
    };

    Ok(Json(PluginListResponse { items, next_cursor }))
}

/// `GET /v1/plugins/{slug}` — latest **published** version.
pub async fn get_one(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<Json<PluginResponse>> {
    require_read(&caller.scope)?;

    let row = sqlx::query!(
        "SELECT id, slug::text AS \"slug!\", version, name, description, manifest, \
                status, sourcing_mode, external_git_url, upstream_url, \
                created_at, updated_at \
         FROM plugins \
         WHERE tenant_id = $1 AND slug = $2 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
        caller.tenant.tenant_id,
        &slug,
    )
    .fetch_optional(state.db_read())
    .await?
    .ok_or(AppError::NotFound)?;

    let content_rows = sqlx::query!(
        "SELECT content_slug, content_kind, content_version, position \
         FROM plugin_contents \
         WHERE plugin_id = $1 \
         ORDER BY position ASC, content_slug ASC",
        row.id,
    )
    .fetch_all(state.db_read())
    .await?;

    let contents = content_rows
        .into_iter()
        .map(|c| PluginContentResponse {
            kind: c.content_kind,
            slug: c.content_slug,
            version: c.content_version,
            position: c.position,
        })
        .collect();

    Ok(Json(PluginResponse {
        slug: row.slug,
        version: row.version,
        name: row.name,
        description: row.description,
        status: row.status,
        sourcing_mode: row.sourcing_mode,
        external_git_url: row.external_git_url,
        upstream_url: row.upstream_url,
        manifest: row.manifest,
        contents,
        created_at: row.created_at,
        updated_at: row.updated_at,
    }))
}

/// `GET /v1/plugins/{slug}/versions` — every version of a plugin slug,
/// newest first. Surfaces all statuses so curators see archived rows too.
pub async fn get_versions(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path(slug): Path<String>,
) -> AppResult<Json<Vec<PluginVersionRow>>> {
    require_read(&caller.scope)?;

    let rows = sqlx::query!(
        "SELECT p.version, p.status, p.created_at, u.email::text AS published_by \
         FROM plugins p \
         LEFT JOIN users u ON u.id = p.created_by \
         WHERE p.tenant_id = $1 AND p.slug = $2 \
         ORDER BY p.created_at DESC \
         LIMIT 50",
        caller.tenant.tenant_id,
        &slug,
    )
    .fetch_all(state.db_read())
    .await?;

    if rows.is_empty() {
        return Err(AppError::NotFound);
    }

    let out = rows
        .into_iter()
        .map(|r| PluginVersionRow {
            version: r.version,
            status: r.status,
            created_at: r.created_at,
            published_by: r.published_by,
        })
        .collect();

    Ok(Json(out))
}

/// `DELETE /v1/plugins/{slug}/versions/{version}` — soft-archive a single
/// version (`status='archived'`). Returns 404 when the row doesn't exist
/// or is already archived (idempotent-DELETE callers should treat 404 as
/// already-done).
pub async fn archive(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Path((slug, version)): Path<(String, String)>,
) -> AppResult<StatusCode> {
    require_publish(&caller.scope)?;

    // RETURNING the previous status lets the audit payload distinguish
    // a published→archived flip from a draft→archived flip. The WHERE
    // clause excludes already-archived rows so the DELETE is naturally
    // idempotent (a second call returns 404).
    let row = sqlx::query!(
        "UPDATE plugins \
         SET status = 'archived', updated_at = now() \
         WHERE tenant_id = $1 AND slug = $2 AND version = $3 AND status <> 'archived' \
         RETURNING id",
        caller.tenant.tenant_id,
        &slug,
        &version,
    )
    .fetch_optional(state.db())
    .await?;

    let Some(_row) = row else {
        return Err(AppError::NotFound);
    };

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: caller.user_id,
            actor_token: Some(caller.token_id),
            action: "plugin.archive",
            target_kind: "plugin",
            target_id: Some(slug.as_str()),
            metadata: serde_json::json!({
                "slug": slug,
                "version": version,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn string_field(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn extract_tags(manifest: &serde_json::Value) -> Vec<String> {
    manifest
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Opaque cursor: base64(<rfc3339-ts>|<uuid>). Stable across pages even
/// when new rows land between requests.
fn encode_cursor(ts: DateTime<Utc>, id: uuid::Uuid) -> String {
    use base64::Engine;
    let raw = format!("{}|{}", ts.to_rfc3339(), id);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

fn decode_cursor(cursor: &str) -> AppResult<(DateTime<Utc>, uuid::Uuid)> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor.as_bytes())
        .map_err(|_| AppError::BadRequest("invalid cursor".into()))?;
    let s = std::str::from_utf8(&raw).map_err(|_| AppError::BadRequest("invalid cursor".into()))?;
    let (ts_str, id_str) = s
        .split_once('|')
        .ok_or_else(|| AppError::BadRequest("invalid cursor".into()))?;
    let ts = DateTime::parse_from_rfc3339(ts_str)
        .map_err(|_| AppError::BadRequest("invalid cursor".into()))?
        .with_timezone(&Utc);
    let id =
        uuid::Uuid::parse_str(id_str).map_err(|_| AppError::BadRequest("invalid cursor".into()))?;
    Ok((ts, id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_round_trip() {
        let ts = Utc::now();
        let id = uuid::Uuid::new_v4();
        let s = encode_cursor(ts, id);
        let (ts2, id2) = decode_cursor(&s).unwrap();
        assert_eq!(id, id2);
        // RFC3339 round-trip preserves to nanosecond precision in chrono.
        assert_eq!(ts.timestamp_micros(), ts2.timestamp_micros());
    }

    #[test]
    fn decode_cursor_rejects_garbage() {
        assert!(decode_cursor("not-base64!!!").is_err());
        // valid base64 but not pipe-separated → 400
        use base64::Engine;
        let bad = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("hello");
        assert!(decode_cursor(&bad).is_err());
    }

    #[test]
    fn extract_tags_handles_missing_and_array() {
        assert_eq!(extract_tags(&serde_json::json!({})), Vec::<String>::new());
        assert_eq!(
            extract_tags(&serde_json::json!({"tags": ["a","b"]})),
            vec!["a".to_string(), "b".to_string()]
        );
        // Wrong type → empty.
        assert_eq!(
            extract_tags(&serde_json::json!({"tags": "not-an-array"})),
            Vec::<String>::new()
        );
    }
}
