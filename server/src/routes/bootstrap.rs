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
    pub stack: String,
    /// When set to `1`/`true`, the response includes a `tier_breakdown`
    /// object showing which slugs each tier contributed (post-dedup).
    /// Off by default to keep the default response shape minimal.
    #[serde(default)]
    pub debug: Option<String>,
}

#[derive(Serialize)]
pub struct BootstrapResponse {
    /// Echoed tags actually used to look up mappings (post-normalisation).
    pub stack: Vec<String>,
    /// Recommended skill slugs in deterministic, tier-priority order.
    pub skills: Vec<String>,
    /// Per-tier attribution. Populated only when `?debug=1`; otherwise
    /// omitted to keep the default response identical to pre-fallback
    /// clients.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tier_breakdown: Option<TierBreakdown>,
}

#[derive(Serialize, Default)]
pub struct TierBreakdown {
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
    if tags.is_empty() {
        return Err(AppError::BadRequest(
            "stack query must contain at least one comma-separated tag".into(),
        ));
    }
    let debug = matches!(q.debug.as_deref(), Some("1") | Some("true"));

    // --- Tier 1: curated --------------------------------------------------
    // Same query as before — every skill mapped to any of the user's stack
    // tags, deduped, capped. Alphabetical order keeps two calls byte-stable.
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
    let curated: Vec<String> = curated_rows.into_iter().map(|(s,)| s).collect();

    // --- Tier 2: tag intersection ----------------------------------------
    // Postgres `&&` is array-overlap; we then count the overlapping tags
    // to surface the broader match first. `cardinality(...)` returns 0
    // when the intersection is empty, so the WHERE clause is what filters
    // — the ORDER BY just ranks the survivors.
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
    let tagged: Vec<String> = tagged_rows.into_iter().map(|(s,)| s).collect();

    // --- Tier 3: semantic similarity -------------------------------------
    // Only when the server has an embedder. NullEmbedder returns None,
    // which we treat as "tier 3 disabled" — NOT an error. Default builds
    // and pgvector-less prod just see an empty contribution here.
    let mut semantic: Vec<String> = Vec::new();
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

    // --- Dedup + cap ------------------------------------------------------
    // Walk tiers in priority order, appending each new slug exactly once.
    // The final `skills` list is the union, the `tier_breakdown` keeps
    // each tier's attributed slice (also post-dedup) for ?debug=1.
    let mut seen: HashSet<String> = HashSet::new();
    let mut all: Vec<String> = Vec::with_capacity(MAX_RESULTS);
    let mut breakdown_curated: Vec<String> = Vec::new();
    let mut breakdown_tagged: Vec<String> = Vec::new();
    let mut breakdown_semantic: Vec<String> = Vec::new();

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
        curated: breakdown_curated,
        tagged: breakdown_tagged,
        semantic: breakdown_semantic,
    });

    Ok(Json(BootstrapResponse {
        stack: tags,
        skills: all,
        tier_breakdown,
    }))
}
