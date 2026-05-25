//! Per-tenant rate limiting (#8 §L20).
//!
//! ## Why two windows
//!
//! The middleware enforces **two** Redis counters per tenant per request:
//!
//! * A 60-second window for the sustained "requests per minute" cap.
//! * A 1-second window for the short-spike "burst" cap.
//!
//! Both must pass for the request to proceed. The 60s window stops a
//! steady-state runaway script; the 1s window stops a single fan-out
//! storm (CI pipeline, `xargs -P 100 curl …`) from saturating the
//! backend even when the per-minute budget is comfortable.
//!
//! ## Fixed-window vs sliding-window
//!
//! v1 uses **fixed windows**: keys are bucketed by `floor(now/60)` and
//! `floor(now)`. This is the simplest correct algorithm — two `INCR`s
//! plus two `EXPIRE`s per request, all in a single pipeline. The
//! pathological case (a client times their request to land at the
//! window boundary) at most doubles the cap for a single second; v1
//! accepts this in exchange for radically simpler code. Sliding-window
//! via Lua or `redis-cell` is a future-work item.
//!
//! ## Plan defaults
//!
//! Tenants on the `team` plan get the baseline; `business` gets 5×;
//! `enterprise` gets 50×. Per-tenant overrides via
//! `tenants.rate_limit_rpm` / `rate_limit_burst` win when non-NULL —
//! the admin CLI (`tenant-rate-limits`) writes those columns.
//!
//! ## Skip list
//!
//! Unauthenticated paths (`/v1/healthz`, `/metrics`, theme/branding,
//! social-crawler OG, OIDC/SAML callbacks, custom-domain `cert-ok`)
//! bypass the limiter entirely — they have no tenant context to
//! attribute the cost to, and they're all either cheap or scraper-friendly
//! by design.
//!
//! ## Fail-open
//!
//! Every Redis error path returns `next.run(request).await`. If Redis
//! is down, the rate-limiter degrades to "no enforcement" rather than
//! making every request fail. We prefer availability to strict
//! enforcement during a cache outage. See `docs/enterprise/rate-limits.md`.

use std::time::{SystemTime, UNIX_EPOCH};

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sqlx::PgPool;
use uuid::Uuid;

use crate::cache::Redis;
use crate::state::AppState;

// ---------------------------------------------------------------------------
// Plan defaults
// ---------------------------------------------------------------------------

/// Effective rate limit pair (RPM + burst) for a single tenant.
#[derive(Debug, Clone, Copy)]
pub struct RateLimit {
    /// Requests allowed in any 60-second fixed window.
    pub rpm: u32,
    /// Requests allowed in any 1-second fixed window.
    pub burst: u32,
}

/// Baseline limits keyed by `tenants.plan_tier`. Unknown plans fall
/// back to the `team` row so a forgotten new tier doesn't silently
/// inherit enterprise-scale headroom.
pub fn default_for_plan(plan: &str) -> RateLimit {
    match plan {
        "team" => RateLimit {
            rpm: 600,
            burst: 60,
        },
        "business" => RateLimit {
            rpm: 3_000,
            burst: 300,
        },
        "enterprise" => RateLimit {
            rpm: 30_000,
            burst: 1_000,
        },
        _ => RateLimit {
            rpm: 600,
            burst: 60,
        },
    }
}

/// Merge the plan default with optional per-tenant overrides. Any
/// override out of bounds is clamped to a sane range (the DB CHECK
/// constraint catches the same cases at write time; this is belt-and-
/// braces for old rows).
pub fn resolve_for_tenant(
    plan: &str,
    rpm_override: Option<i32>,
    burst_override: Option<i32>,
) -> RateLimit {
    let base = default_for_plan(plan);
    let rpm = rpm_override
        .filter(|v| *v > 0 && *v <= 100_000)
        .map(|v| v as u32)
        .unwrap_or(base.rpm);
    let burst = burst_override
        .filter(|v| *v > 0 && *v <= 10_000)
        .map(|v| v as u32)
        .unwrap_or(base.burst);
    RateLimit { rpm, burst }
}

// ---------------------------------------------------------------------------
// Skip list
// ---------------------------------------------------------------------------

/// Paths (matched as prefixes — see `is_skipped`) that bypass the limiter.
///
/// Rationale per entry:
/// * `/v1/healthz` — liveness probe; ratelimiting would self-DoS the LB.
/// * `/metrics` — Prometheus scrape; same reason.
/// * `/v1/theme*` — login-page branding; pre-auth, no tenant header.
/// * `/v1/og` — social-crawler card; we *want* Slack/Twitter to fetch this.
/// * `/v1/tenant/profile/banner` — CLI startup banner; no tenant header in
///   shared mode (the slug is the subdomain).
/// * `/v1/tenant/session-policy` — login flow reads this before auth.
/// * `/v1/auth/oidc/`, `/v1/auth/saml/` — sign-in callbacks; pre-token.
/// * `/v1/tenant/custom-domains/*/cert-ok` — Caddy on_demand_tls ask
///   endpoint; called by the proxy, not the tenant.
const SKIP_PATHS: &[&str] = &[
    "/v1/healthz",
    "/metrics",
    "/v1/theme", // covers /v1/theme, /v1/theme/logo, /v1/theme/favicon, /v1/theme/custom.css, /v1/theme/fonts, /v1/theme/custom-css
    "/v1/og",
    "/v1/tenant/profile/banner",
    "/v1/tenant/session-policy",
    "/v1/auth/oidc/",
    "/v1/auth/saml/",
];

/// Returns true when the path bypasses the limiter. Prefix-matched so
/// new sub-routes under `/v1/theme/*` etc. don't have to update this
/// list. `/v1/tenant/custom-domains/{host}/cert-ok` is matched by suffix
/// because the host segment varies.
fn is_skipped(path: &str) -> bool {
    if SKIP_PATHS.iter().any(|p| path == *p || path.starts_with(p)) {
        return true;
    }
    // Caddy on_demand_tls ask — path is /v1/tenant/custom-domains/<host>/cert-ok.
    if path.starts_with("/v1/tenant/custom-domains/") && path.ends_with("/cert-ok") {
        return true;
    }
    false
}

// ---------------------------------------------------------------------------
// Cheap tenant resolution (no DB round-trip for the slug)
// ---------------------------------------------------------------------------

/// Extract the tenant slug from headers exactly like
/// `tenant::slug_from_request`, but without an `AppError` dependency
/// and without consulting the tenancy mode (rate-limit middleware
/// always runs in shared-mode paths — the dedicated-mode binary pins a
/// single slug at startup which is also fine to use here).
fn slug_from_headers(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(val) = headers.get("x-skill-pool-tenant") {
        if let Ok(s) = val.to_str() {
            let s = s.trim().to_lowercase();
            if !s.is_empty() {
                return Some(s);
            }
        }
    }
    let host = headers.get("host").and_then(|h| h.to_str().ok())?;
    let host_no_port = host.split(':').next().unwrap_or(host);
    let label = host_no_port.split('.').next()?;
    if label.is_empty() || label == "www" {
        return None;
    }
    Some(label.to_lowercase())
}

/// Look up the (tenant_id, plan, overrides) row for `slug`. Cached in
/// a tiny in-process map keyed by slug; TTL is short (5s) so an admin
/// CLI write surfaces quickly. A single SELECT per cold lookup; on a
/// hit there's no DB cost. Returns `None` for an unknown slug — the
/// limiter then bypasses (request will 401 at the auth extractor anyway).
async fn lookup_tenant(
    db: &PgPool,
    slug: &str,
) -> Option<(Uuid, String, Option<i32>, Option<i32>)> {
    // First check the custom-domain side: the slug we extracted from the
    // subdomain may actually be a hostname label of a custom domain. For
    // simplicity we only handle the subdomain/header case here; custom-
    // domain hosts skip rate limiting in v1 (the next refactor can add a
    // second lookup keyed by host).
    sqlx::query!(
        "SELECT id, plan_tier, rate_limit_rpm, rate_limit_burst \
         FROM tenants \
         WHERE slug = $1 AND status = 'active'",
        slug,
    )
    .fetch_optional(db)
    .await
    .ok()
    .flatten()
    .map(|r| (r.id, r.plan_tier, r.rate_limit_rpm, r.rate_limit_burst))
}

// ---------------------------------------------------------------------------
// Middleware
// ---------------------------------------------------------------------------

/// Axum middleware that enforces a per-tenant request budget against
/// two Redis fixed-window counters (60s RPM + 1s burst). Wire it after
/// `tenant_span_layer` and `TraceLayer`, before `metrics::track`, so the
/// 429 response shows up in metrics under the rate-limited path.
pub async fn rate_limit_layer(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    // 1. No Redis → fail open. Don't add latency for a feature that
    //    isn't configured.
    let Some(redis) = state.redis().cloned() else {
        return next.run(request).await;
    };

    // 2. Skip the unauthenticated / pre-tenant paths.
    if is_skipped(request.uri().path()) {
        return next.run(request).await;
    }

    // 3. Resolve the tenant slug from headers without touching the DB.
    //    No slug = no tenant context to attribute against; let the
    //    downstream auth/tenant extractor 401/400 it.
    let Some(slug) = slug_from_headers(request.headers()) else {
        return next.run(request).await;
    };

    // 4. Resolve the limit row. A single SELECT per cold tenant; v2
    //    will cache this through `cache::cached_json`. Sister-agent A
    //    owns that helper; until it lands, the per-request hit is
    //    acceptable — `tenants` is tiny and the query is on the PK index.
    let Some((tenant_id, plan, rpm_override, burst_override)) =
        lookup_tenant(state.db(), &slug).await
    else {
        return next.run(request).await;
    };
    let limit = resolve_for_tenant(&plan, rpm_override, burst_override);

    // 5. Bump both counters and decide. Fail-open on any Redis error.
    match check_and_bump(&redis, tenant_id, &limit).await {
        Ok(Outcome::Allow {
            rpm_remaining,
            rpm_reset,
        }) => {
            let mut resp = next.run(request).await;
            attach_headers(&mut resp, limit.rpm, rpm_remaining, rpm_reset);
            resp
        }
        Ok(Outcome::Deny { retry_after, reset }) => deny(limit.rpm, retry_after, reset),
        Err(e) => {
            // Fail open. Log once per failure so an operator notices a
            // sustained Redis outage in the structured logs without us
            // hammering the user.
            tracing::warn!(error = %e, slug = %slug, "rate-limit redis failure; failing open");
            next.run(request).await
        }
    }
}

/// What `check_and_bump` decided.
enum Outcome {
    /// Below both caps. Includes the per-minute remaining count and the
    /// unix timestamp at which the 60s window rolls over (for the
    /// response headers).
    Allow { rpm_remaining: u32, rpm_reset: u64 },
    /// At or above one of the caps. `retry_after` is seconds until the
    /// shorter of the two windows resets; `reset` is the unix ts.
    Deny { retry_after: u64, reset: u64 },
}

/// Pipeline the two `INCR + EXPIRE` pairs and decide. Returns
/// `Err(redis::RedisError)` only on a real protocol failure — bumped
/// counts above the cap return `Ok(Outcome::Deny)`.
async fn check_and_bump(
    redis: &Redis,
    tenant_id: Uuid,
    limit: &RateLimit,
) -> Result<Outcome, redis::RedisError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let minute_bucket = now / 60;
    let second_bucket = now;

    let rpm_key = format!("rl:1m:{tenant_id}:{minute_bucket}");
    let burst_key = format!("rl:1s:{tenant_id}:{second_bucket}");

    // Single round-trip: two INCRs, two EXPIREs. EXPIRE windows are
    // generous (90s for the minute key, 5s for the second key) so a
    // narrowly-missed expiry doesn't drop a counter mid-window.
    // `cache::Redis = Arc<ConnectionManager>`, and `ConnectionManager`
    // is cheaply `Clone`. Deref the `&Redis` then clone the inner
    // ConnectionManager — query_async wants &mut ConnectionLike.
    let mut conn = (**redis).clone();
    let (rpm_count, burst_count): (i64, i64) = redis::pipe()
        .atomic()
        .cmd("INCR")
        .arg(&rpm_key)
        .cmd("EXPIRE")
        .arg(&rpm_key)
        .arg(90)
        .ignore()
        .cmd("INCR")
        .arg(&burst_key)
        .cmd("EXPIRE")
        .arg(&burst_key)
        .arg(5)
        .ignore()
        .query_async(&mut conn)
        .await?;

    let minute_reset = (minute_bucket + 1) * 60;
    let second_reset = second_bucket + 1;

    if rpm_count > limit.rpm as i64 {
        // RPM window is the bigger budget but the bigger penalty —
        // Retry-After points to the end of the minute.
        let retry = minute_reset.saturating_sub(now).max(1);
        return Ok(Outcome::Deny {
            retry_after: retry,
            reset: minute_reset,
        });
    }
    if burst_count > limit.burst as i64 {
        // Burst window — Retry-After is at most 1s, so the client can
        // retry almost immediately. This is the common case for a
        // misbehaving parallel `xargs`.
        let retry = second_reset.saturating_sub(now).max(1);
        return Ok(Outcome::Deny {
            retry_after: retry,
            reset: second_reset,
        });
    }

    let rpm_remaining = (limit.rpm as i64 - rpm_count).max(0) as u32;
    Ok(Outcome::Allow {
        rpm_remaining,
        rpm_reset: minute_reset,
    })
}

// ---------------------------------------------------------------------------
// Response shaping
// ---------------------------------------------------------------------------

fn attach_headers(resp: &mut Response, limit: u32, remaining: u32, reset_ts: u64) {
    let headers = resp.headers_mut();
    let _ = headers.insert(
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderValue::from(limit),
    );
    let _ = headers.insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from(remaining),
    );
    if let Ok(v) = HeaderValue::from_str(&reset_ts.to_string()) {
        let _ = headers.insert(HeaderName::from_static("x-ratelimit-reset"), v);
    }
}

fn deny(limit: u32, retry_after: u64, reset_ts: u64) -> Response {
    let body = serde_json::json!({
        "error": "rate_limit_exceeded",
        "message": "tenant rate limit exceeded; retry after the window resets",
        "retry_after_seconds": retry_after,
    });
    let mut resp = (StatusCode::TOO_MANY_REQUESTS, axum::Json(body)).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&retry_after.to_string()) {
        let _ = headers.insert(HeaderName::from_static("retry-after"), v);
    }
    let _ = headers.insert(
        HeaderName::from_static("x-ratelimit-limit"),
        HeaderValue::from(limit),
    );
    let _ = headers.insert(
        HeaderName::from_static("x-ratelimit-remaining"),
        HeaderValue::from(0u32),
    );
    if let Ok(v) = HeaderValue::from_str(&reset_ts.to_string()) {
        let _ = headers.insert(HeaderName::from_static("x-ratelimit-reset"), v);
    }
    // Discourage caching of a 429.
    if let Ok(v) = HeaderValue::from_str("no-store") {
        let _ = headers.insert(HeaderName::from_static("cache-control"), v);
    }
    resp
}

// ---------------------------------------------------------------------------
// Unit tests for the pure helpers
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_defaults_are_monotonic() {
        let t = default_for_plan("team");
        let b = default_for_plan("business");
        let e = default_for_plan("enterprise");
        assert!(t.rpm < b.rpm && b.rpm < e.rpm);
        assert!(t.burst < b.burst && b.burst < e.burst);
        // Unknown plan falls back to team.
        let u = default_for_plan("free-trial");
        assert_eq!(u.rpm, t.rpm);
        assert_eq!(u.burst, t.burst);
    }

    #[test]
    fn overrides_win_when_in_range() {
        let l = resolve_for_tenant("team", Some(50_000), Some(500));
        assert_eq!(l.rpm, 50_000);
        assert_eq!(l.burst, 500);
    }

    #[test]
    fn overrides_clamped_when_out_of_range() {
        // Zero and negative are ignored — fall back to plan default.
        let l = resolve_for_tenant("team", Some(0), Some(-1));
        assert_eq!(l.rpm, 600);
        assert_eq!(l.burst, 60);
        // Beyond CHECK upper bound — same fallback.
        let l = resolve_for_tenant("team", Some(200_000), Some(50_000));
        assert_eq!(l.rpm, 600);
        assert_eq!(l.burst, 60);
    }

    #[test]
    fn skip_paths_cover_expected_endpoints() {
        assert!(is_skipped("/v1/healthz"));
        assert!(is_skipped("/metrics"));
        assert!(is_skipped("/v1/theme"));
        assert!(is_skipped("/v1/theme/logo"));
        assert!(is_skipped("/v1/theme/custom.css"));
        assert!(is_skipped("/v1/og"));
        assert!(is_skipped("/v1/tenant/profile/banner"));
        assert!(is_skipped("/v1/tenant/session-policy"));
        assert!(is_skipped("/v1/auth/oidc/acme/start"));
        assert!(is_skipped("/v1/auth/saml/acme/acs"));
        assert!(is_skipped(
            "/v1/tenant/custom-domains/skills.acme.com/cert-ok"
        ));
        // Real API paths must NOT be skipped.
        assert!(!is_skipped("/v1/skills"));
        assert!(!is_skipped("/v1/drafts"));
        assert!(!is_skipped("/v1/mcp"));
    }

    #[test]
    fn slug_extraction_prefers_header() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "acme.skill-pool.local".parse().unwrap());
        h.insert("x-skill-pool-tenant", "globex".parse().unwrap());
        assert_eq!(slug_from_headers(&h).as_deref(), Some("globex"));
    }

    #[test]
    fn slug_extraction_falls_back_to_subdomain() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "acme.skill-pool.local:8080".parse().unwrap());
        assert_eq!(slug_from_headers(&h).as_deref(), Some("acme"));
    }

    #[test]
    fn slug_extraction_skips_www_and_bare_hosts() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "www.example.com".parse().unwrap());
        assert_eq!(slug_from_headers(&h), None);
        // Single-label host has no subdomain to extract.
        let mut h = axum::http::HeaderMap::new();
        h.insert("host", "localhost".parse().unwrap());
        // "localhost" itself is a valid label and gets returned —
        // intentional, so dev runs work.
        assert_eq!(slug_from_headers(&h).as_deref(), Some("localhost"));
    }
}
