//! Bootstrap endpoint — given a stack fingerprint, return up to 8 skill
//! slugs the developer should ship with.
//!
//! Three tiers, unioned in this order (highest precision first):
//!
//!   1. **Curated** — `tenant_stack_mappings` rows the admin maintains.
//!      Highest precision; this is the team's intentional shape.
//!   2. **Tag intersection** — published skills in this tenant whose
//!      `tags` array overlaps the stack tags. Orders by overlap-count
//!      DESC, then created_at DESC so the freshest broad-matches surface.
//!   3. **Semantic similarity** — embed the joined stack string and rank
//!      published skills by cosine distance over their description
//!      embeddings. Skipped entirely on `NullEmbedder` (default build),
//!      so dev workstations and pgvector-less prod degrade gracefully.
//!
//! Dedup priority is curated > tagged > semantic: a slug surfaced by a
//! higher tier is removed from lower tiers' results before the union.
//! The final response is capped at [`MAX_RESULTS`] in tier order.
//!
//! The master plan calls for an LLM-fallback as a fourth tier, but we
//! ship embedding-similarity (tier 3) instead and skip LLM entirely:
//!   - the embedding infra is already wired (`fastembed` Cargo feature,
//!     `description_embedding` column on every published skill, HNSW
//!     index for fast ranking);
//!   - it's deterministic and free at request time, vs. an LLM call that
//!     costs tokens and requires an Anthropic API key on the server;
//!   - LLM-fallback is more useful for *describing a new repo* than for
//!     *picking from an existing catalog*, which is what bootstrap does.

use std::collections::HashSet;

use axum::extract::{Query, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const MAX_RESULTS: usize = 8;

/// Internal cap on rows pulled from each tier before dedup-and-cap. A
/// little larger than `MAX_RESULTS` so we still fill the response when
/// the top hits from one tier are all dedup-shadowed by an earlier tier.
const PER_TIER_FETCH: i64 = MAX_RESULTS as i64 * 2;

#[derive(Deserialize)]
pub struct BootstrapQuery {
    /// Comma-separated stack tags.
    #[serde(default)]
    pub stack: String,
    /// Optional project slug. When set, project items are fetched first
    /// (tier 0, highest precedence) and existing tiers backfill remaining
    /// slots up to the 8-cap.
    #[serde(default)]
    pub project: Option<String>,
    /// When set to `1`/`true`, the response includes a `tier_breakdown`
    /// object showing which slugs each tier contributed (post-dedup).
    /// Off by default to keep the default response shape minimal.
    #[serde(default)]
    pub debug: Option<String>,
}

#[derive(Serialize)]
pub struct ProjectRef {
    pub slug: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct BootstrapResponse {
    /// Echoed tags actually used to look up mappings (post-normalisation).
    pub stack: Vec<String>,
    /// Recommended skill slugs in deterministic, tier-priority order.
    pub skills: Vec<String>,
    /// The project that contributed tier-0 items, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<ProjectRef>,
    /// Per-tier attribution. Populated only when `?debug=1`; otherwise
    /// omitted to keep the default response identical to pre-fallback
    /// clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_breakdown: Option<TierBreakdown>,
}

#[derive(Serialize, Default)]
pub struct TierBreakdown {
    /// Slugs sourced from a named project's item list (tier 0).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub project: Vec<String>,
    /// Slugs sourced from `tenant_stack_mappings`.
    pub curated: Vec<String>,
    /// Slugs surfaced by tag-array overlap (after dedup against curated).
    pub tagged: Vec<String>,
    /// Slugs surfaced by embedding similarity (after dedup against the
    /// two higher tiers). Empty when no embedder is configured.
    pub semantic: Vec<String>,
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
    let debug = matches!(q.debug.as_deref(), Some("1") | Some("true"));

    // At least one of stack or project must be provided.
    if tags.is_empty() && q.project.is_none() {
        return Err(AppError::BadRequest(
            "stack query must contain at least one comma-separated tag (or supply ?project=)".into(),
        ));
    }

    // --- Tier 0: project items -------------------------------------------
    // Highest precedence. When a project slug is provided, fetch its items
    // in curator-defined position order. A non-existent project slug is a
    // soft miss (no error — fall through to stack tiers) so that a stale
    // manifest entry doesn't hard-fail a developer's bootstrap.
    let mut project_ref: Option<ProjectRef> = None;
    let mut project_items: Vec<String> = Vec::new();

    if let Some(ref proj_slug) = q.project {
        let proj_slug = proj_slug.trim();
        if !proj_slug.is_empty() {
            let result = crate::admin::get_project(
                state.db_read(),
                &caller.tenant.tenant_slug,
                proj_slug,
            )
            .await
            .map_err(AppError::Anyhow)?;

            if let Some(pw) = result {
                project_ref = Some(ProjectRef {
                    slug: pw.project.slug.clone(),
                    name: pw.project.name.clone(),
                });
                project_items = pw
                    .items
                    .into_iter()
                    .map(|i| i.skill_slug)
                    .collect();
            }
            // Non-existent project → soft fall-through (project_ref stays None)
        }
    }

    // --- Tier 1: curated --------------------------------------------------
    // Same query as before — every skill mapped to any of the user's stack
    // tags, deduped, capped. Alphabetical order keeps two calls byte-stable.
    // Skipped when the tags list is empty (project-only bootstrap).
    let curated: Vec<String> = if !tags.is_empty() {
        let curated_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT skill_slug \
             FROM tenant_stack_mappings \
             WHERE tenant_id = $1 AND stack_tag = ANY($2) \
             ORDER BY skill_slug \
             LIMIT $3",
        )
        .bind(caller.tenant.tenant_id)
        .bind(&tags)
        .bind(PER_TIER_FETCH)
        .fetch_all(state.db_read())
        .await?;
        curated_rows.into_iter().map(|(s,)| s).collect()
    } else {
        Vec::new()
    };

    // --- Tier 2: tag intersection ----------------------------------------
    // Postgres `&&` is array-overlap; we then count the overlapping tags
    // to surface the broader match first. `cardinality(...)` returns 0
    // when the intersection is empty, so the WHERE clause is what filters
    // — the ORDER BY just ranks the survivors.
    let tagged: Vec<String> = if !tags.is_empty() {
        let tagged_rows: Vec<(String,)> = sqlx::query_as(
            "SELECT DISTINCT ON (slug) slug \
             FROM ( \
               SELECT slug, created_at, \
                      cardinality(ARRAY( \
                        SELECT unnest(tags) INTERSECT SELECT unnest($2::text[]) \
                      )) AS overlap_count \
               FROM skills \
               WHERE tenant_id = $1 \
                 AND kind = 'skill' \
                 AND status = 'published' \
                 AND tags && $2::text[] \
             ) ranked \
             ORDER BY slug, overlap_count DESC, created_at DESC \
             LIMIT $3",
        )
        .bind(caller.tenant.tenant_id)
        .bind(&tags)
        .bind(PER_TIER_FETCH)
        .fetch_all(state.db_read())
        .await?;
        tagged_rows.into_iter().map(|(s,)| s).collect()
    } else {
        Vec::new()
    };

    // --- Tier 3: semantic similarity -------------------------------------
    // Only when the server has an embedder. NullEmbedder returns None,
    // which we treat as "tier 3 disabled" — NOT an error. Default builds
    // and pgvector-less prod just see an empty contribution here.
    let mut semantic: Vec<String> = Vec::new();
    if !tags.is_empty() {
        let joined = tags.join(" ");
        let embedding = state
            .embedder()
            .embed(&joined)
            .map_err(AppError::Anyhow)?;
        if let Some(vec) = embedding {
            let lit = crate::embedding::vector_to_pg_literal(&vec);
            let rows: Vec<(String,)> = sqlx::query_as(
                "SELECT slug FROM ( \
                    SELECT DISTINCT ON (slug) slug, created_at, description_embedding \
                    FROM skills \
                    WHERE tenant_id = $1 \
                      AND kind = 'skill' \
                      AND status = 'published' \
                      AND description_embedding IS NOT NULL \
                    ORDER BY slug, created_at DESC \
                 ) latest \
                 ORDER BY description_embedding <=> $2::text::vector ASC \
                 LIMIT $3",
            )
            .bind(caller.tenant.tenant_id)
            .bind(lit)
            .bind(PER_TIER_FETCH)
            .fetch_all(state.db_read())
            .await?;
            semantic = rows.into_iter().map(|(s,)| s).collect();
        }
    }

    // --- Dedup + cap ------------------------------------------------------
    // Walk tiers in priority order (0 → 1 → 2 → 3), appending each new
    // slug exactly once. The final `skills` list is the union, the
    // `tier_breakdown` keeps each tier's attributed slice (post-dedup) for
    // ?debug=1.
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::with_capacity(MAX_RESULTS);
    let mut breakdown_project: Vec<String> = Vec::new();
    let mut breakdown_curated: Vec<String> = Vec::new();
    let mut breakdown_tagged: Vec<String> = Vec::new();
    let mut breakdown_semantic: Vec<String> = Vec::new();

    for slug in &project_items {
        if all.len() >= MAX_RESULTS {
            break;
        }
        if seen.insert(slug.clone()) {
            all.push(slug.clone());
            breakdown_project.push(slug.clone());
        }
    }
    for slug in &curated {
        if all.len() >= MAX_RESULTS {
            break;
        }
        if seen.insert(slug.clone()) {
            all.push(slug.clone());
            breakdown_curated.push(slug.clone());
        }
    }
    for slug in &tagged {
        if all.len() >= MAX_RESULTS {
            break;
        }
        if seen.insert(slug.clone()) {
            all.push(slug.clone());
            breakdown_tagged.push(slug.clone());
        }
    }
    for slug in &semantic {
        if all.len() >= MAX_RESULTS {
            break;
        }
        if seen.insert(slug.clone()) {
            all.push(slug.clone());
            breakdown_semantic.push(slug.clone());
        }
    }

    let tier_breakdown = debug.then_some(TierBreakdown {
        project: breakdown_project,
        curated: breakdown_curated,
        tagged: breakdown_tagged,
        semantic: breakdown_semantic,
    });

    Ok(Json(BootstrapResponse {
        stack: tags,
        skills: all,
        project: project_ref,
        tier_breakdown,
    }))
}
