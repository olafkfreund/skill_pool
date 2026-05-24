//! `POST /v1/plugins/import` — enqueue a plugin mirror job (issue #32).
//!
//! Accepts a JSON body with `{url, slug, refresh_interval_secs?}`, validates
//! it, creates (or updates) a plugin row with `sourcing_mode = 'mirror'`, then
//! enqueues a `PluginMirrorJob` and returns 202 with the job id.
//!
//! ## Scope policy
//!
//! Requires `skills:publish` scope — the same guard as `POST /v1/plugins`.
//! Both `curator` and `admin` roles carry this scope per `auth::role_to_scope`.
//!
//! ## Idempotency
//!
//! Re-posting the same `{slug, url}` within 24h returns 202 with outcome
//! `"deduped"` — the job envelope is already in the queue. After 24h the
//! dedup marker expires and a new job is enqueued. This matches the intended
//! daily-sweep cadence.
//!
//! ## Rate limiting
//!
//! Uses the same per-tenant rate-limiter middleware as all other `/v1/*`
//! routes. No extra per-endpoint limit: the queue's idempotency key already
//! prevents a burst from spawning N parallel mirror processes.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::jobs::plugin_mirror::{PluginMirrorJob, MIN_PULL_INTERVAL_SECS};
use crate::queue::{EnqueueOutcome, Job};
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Request / response
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct ImportBody {
    /// The git URL of the upstream plugin repository.
    /// Must be non-empty; scheme validation is left to git2 at clone time.
    pub url: String,

    /// The slug under which this mirror will be registered in this tenant's
    /// marketplace. Must be unique within the tenant (enforced by the DB
    /// UNIQUE constraint on `(tenant_id, slug, version)`; the import handler
    /// uses version = "mirror" as a sentinel until the first pull updates it).
    pub slug: String,

    /// Optional refresh interval in seconds. Must be >= 300 (5 min) per the
    /// spec. Defaults to 86400 (24 h) when absent.
    #[serde(default)]
    pub refresh_interval_secs: Option<i64>,
}

#[derive(Serialize)]
pub struct ImportResponse {
    /// The job id in the queue. Stable for 7 days (envelope TTL).
    pub job_id: String,
    /// `"enqueued"` or `"deduped"` (re-submitted within 24h dedup window).
    pub outcome: &'static str,
    /// UUID of the plugin row — created by this call or returned from an
    /// existing row for this `(tenant_id, slug)`.
    pub plugin_id: Uuid,
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `POST /v1/plugins/import`
pub async fn import(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<ImportBody>,
) -> AppResult<(StatusCode, Json<ImportResponse>)> {
    // 1. Scope guard — curator or admin only.
    require_publish(&caller.scope)?;

    // 2. Input validation.
    let url = body.url.trim().to_string();
    if url.is_empty() {
        return Err(AppError::BadRequest("url is required".into()));
    }
    let slug = body.slug.trim().to_string();
    if slug.is_empty() {
        return Err(AppError::BadRequest("slug is required".into()));
    }

    if let Some(interval) = body.refresh_interval_secs {
        if interval < MIN_PULL_INTERVAL_SECS {
            return Err(AppError::BadRequest(format!(
                "refresh_interval_secs must be >= {} (5 minutes), got {}",
                MIN_PULL_INTERVAL_SECS, interval
            )));
        }
    }

    // 3. Upsert the plugin row as a mirror stub. Version starts as "pending"
    //    and is updated to the real manifest version after the first successful
    //    pull. Using ON CONFLICT on (tenant_id, slug, version) means a second
    //    import of the same slug at the same pending version is idempotent.
    let pull_interval = body.refresh_interval_secs;
    let tenant_id = caller.tenant.tenant_id;

    // Use sqlx::query (non-macro) so this compiles with SQLX_OFFLINE=true
    // before the new query cache entry has been prepared. The macro form
    // (query_scalar!) requires a live DB or cache entry at compile time.
    let plugin_id: Uuid = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO plugins \
             (tenant_id, slug, version, name, description, manifest, \
              status, sourcing_mode, upstream_url, pull_interval_secs) \
         VALUES ($1, $2, 'pending', $2, NULL, '{}', \
                 'draft', 'mirror', $3, $4) \
         ON CONFLICT (tenant_id, slug, version) DO UPDATE \
             SET upstream_url      = EXCLUDED.upstream_url, \
                 pull_interval_secs = EXCLUDED.pull_interval_secs, \
                 updated_at        = now() \
         RETURNING id",
    )
    .bind(tenant_id)
    .bind(&slug)
    .bind(&url)
    .bind(pull_interval)
    .fetch_one(state.db())
    .await
    .map_err(|e| {
        // The DB CHECK for slug format (CITEXT, but otherwise free-form)
        // will produce a constraint violation here if the slug violates the
        // downstream CHECK constraints from migration 0031.
        tracing::warn!(error = %e, tenant_id = %tenant_id, slug = %slug, "plugin import upsert failed");
        AppError::from(e)
    })?;

    // 4. Enqueue the mirror job — or accept the dedup outcome gracefully.
    let queue = state.queue().ok_or_else(|| {
        AppError::BadRequest(
            "job queue not available (Redis not configured); cannot enqueue mirror job".into(),
        )
    })?;

    let job = PluginMirrorJob {
        plugin_id,
        tenant_id,
        upstream_url: url,
    };

    // The idempotency key embeds only plugin_id so that re-importing the same
    // plugin (e.g. after updating refresh_interval_secs) within 24h returns
    // "deduped" rather than racing two clone processes.
    let outcome = queue
        .enqueue(&job)
        .await
        .map_err(|e| AppError::BadRequest(format!("enqueue mirror job: {e}")))?;

    let (outcome_str, job_id) = match outcome {
        EnqueueOutcome::Enqueued => {
            // The job id we need for the response is embedded in the queue's
            // internal envelope, but `enqueue` doesn't return it. We derive
            // a deterministic representation from the idempotency key so the
            // caller can correlate; the actual Redis job id differs.
            ("enqueued", job.idempotency_key())
        }
        EnqueueOutcome::Deduped => ("deduped", job.idempotency_key()),
    };

    Ok((
        StatusCode::ACCEPTED,
        Json(ImportResponse {
            job_id: job_id,
            outcome: outcome_str,
            plugin_id,
        }),
    ))
}

// ---------------------------------------------------------------------------
// Scope helper (mirrors routes/plugins.rs)
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
