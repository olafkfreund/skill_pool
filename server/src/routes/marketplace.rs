//! Per-tenant `/.claude-plugin/marketplace.json` endpoint (#31).
//!
//! The catalogue Claude Code consumes via `/plugin marketplace add <url>`.
//! Schema follows the spec at
//! <https://code.claude.com/docs/en/plugin-marketplaces#marketplace-schema>:
//! a JSON object with `name`, `owner`, and `plugins[]` — one entry per
//! published plugin slug in this tenant.
//!
//! ## Public read, rate-limited
//!
//! No `AuthedCaller` extractor — `/plugin marketplace add` is unauthenticated
//! by design (the user opts in by pasting the URL, mirroring how a `git
//! clone` to a public repo works). The per-tenant rate limiter still
//! applies because this route is **not** in `rate_limit::SKIP_PATHS`.
//!
//! ## Entry assembly
//!
//! Each plugin row contributes one entry. We pre-render the JSON object per
//! plugin into `plugin_marketplace_entries.entry_json` at publish time (via
//! `regenerate_entry`) so this handler is a single tenant-scoped SELECT
//! plus a JSON shim around the rows — no per-request manifest parsing.
//!
//! ## Cache headers
//!
//! `ETag` is the lowercase hex sha256 of the response body; `Cache-Control:
//! public, max-age=60` matches the auth-cache TTL so admins see updates
//! within a minute without us re-rendering JSON on every Claude Code poll.

use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

/// `GET /.claude-plugin/marketplace.json` — Claude Code marketplace catalogue.
pub async fn get_marketplace(
    State(state): State<AppState>,
    tenant: TenantCtx,
    headers: HeaderMap,
) -> AppResult<Response> {
    // Owner display name + URL. Falls back to the slug when the row is
    // missing somehow (shouldn't happen — TenantCtx already resolved it,
    // but a between-request tenant delete is theoretically possible).
    let tenant_row = sqlx::query!(
        "SELECT name FROM tenants WHERE id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db_read())
    .await?
    .ok_or(AppError::NotFound)?;
    let owner_name = if tenant_row.name.is_empty() {
        tenant.tenant_slug.clone()
    } else {
        tenant_row.name
    };

    let entries = sqlx::query!(
        "SELECT entry_json \
         FROM plugin_marketplace_entries \
         WHERE tenant_id = $1 \
         ORDER BY plugin_slug ASC",
        tenant.tenant_id,
    )
    .fetch_all(state.db_read())
    .await?;

    let plugins: Vec<Value> = entries.into_iter().map(|r| r.entry_json).collect();

    // Owner.url points at the public marketplace browser page (lands in
    // #34/#7). We emit it now so the marketplace.json shape is stable from
    // day one; the page returns 404 until that PR ships, which Claude Code
    // tolerates (it's metadata, not load-bearing).
    let body = json!({
        "name": tenant.tenant_slug,
        "owner": {
            "name": owner_name,
            "url": format!("{}/marketplace", origin_from_request(&headers)),
        },
        "plugins": plugins,
    });

    let body_bytes = serde_json::to_vec(&body)
        .map_err(|e| AppError::BadRequest(format!("marketplace.json serialise: {e}")))?;
    let etag = compute_etag(&body_bytes);

    // Conditional GET — return 304 when the client already has the same
    // ETag. Saves bandwidth on the per-minute Claude Code refresh loop.
    if let Some(prev) = headers.get(IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if prev.trim() == etag {
            return Ok((StatusCode::NOT_MODIFIED, [(ETAG, etag)]).into_response());
        }
    }

    let mut resp = (StatusCode::OK, body_bytes).into_response();
    let h = resp.headers_mut();
    let _ = h.insert(CONTENT_TYPE, "application/json".parse().unwrap());
    let _ = h.insert(ETAG, etag.parse().unwrap());
    let _ = h.insert(CACHE_CONTROL, "public, max-age=60".parse().unwrap());
    Ok(resp)
}

/// Strong-ETag derived from the body — sha256 hex prefixed and quoted per
/// RFC 7232. Lowercase hex keeps `If-None-Match` comparisons trivially
/// case-sensitive.
fn compute_etag(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let hex = hex::encode(h.finalize());
    format!("\"{}\"", &hex[..32])
}

/// Reconstruct the public origin (`scheme://host`) from request headers so
/// `source.url` and `owner.url` point at the URL the caller actually used.
/// Falls back to `http` when no proto header is present (test mode); prod
/// terminates TLS at the reverse proxy which sets `X-Forwarded-Proto: https`.
/// Re-exported so the plugins publish handler can use the same logic when
/// it pre-renders a marketplace entry.
pub(crate) fn origin_from_request(headers: &HeaderMap) -> String {
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http".to_string());
    let host = headers
        .get("host")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "localhost".to_string());
    format!("{scheme}://{host}")
}

// ---------------------------------------------------------------------------
// Hook called from routes/plugins.rs::publish
// ---------------------------------------------------------------------------

/// Upsert the pre-rendered entry for a single plugin slug into
/// `plugin_marketplace_entries`. Idempotent — replaces any prior entry
/// for the same `(tenant, slug)`.
///
/// Source URL derivation:
///   * `internal` / `mirror` → `<origin>/git/plugins/<slug>.git` (we host)
///   * `external` → `external_git_url` verbatim, with a github-form shortcut
///     when the URL is a github.com repo.
///
/// Origin is read from the request headers passed in by the caller —
/// publish handlers have access to those via `axum::http::HeaderMap`.
///
/// **Idempotent.** The underlying `INSERT ... ON CONFLICT
/// (tenant_id, plugin_slug) DO UPDATE` makes this safe to retry after a
/// partial failure of the post-publish hook. Two back-to-back calls with
/// identical input leave the marketplace row in the same end state — one
/// row per `(tenant, slug)` whose contents reflect the latest call. Pair
/// with `plugin_git::materialise_internal`'s tree-SHA short-circuit so a
/// failed-then-retried publish converges fully.
#[allow(clippy::too_many_arguments)]
pub async fn regenerate_entry(
    state: &AppState,
    tenant: &TenantCtx,
    slug: &str,
    version: &str,
    plugin_id: Uuid,
    sourcing_mode: &str,
    external_git_url: Option<&str>,
    manifest: &Value,
    origin: &str,
) -> AppResult<()> {
    let source = build_source(sourcing_mode, external_git_url, origin, slug)?;

    // Keep entry_json shape narrow — only fields Claude Code reads. The
    // marketplace browser (#34/#7) reads richer detail via /v1/plugins/{slug}.
    let mut entry = serde_json::Map::new();
    entry.insert("name".into(), Value::String(slug.to_string()));
    if let Some(desc) = manifest.get("description").and_then(|v| v.as_str()) {
        entry.insert("description".into(), Value::String(desc.to_string()));
    }
    entry.insert("version".into(), Value::String(version.to_string()));
    entry.insert("source".into(), source.clone());
    if let Some(kw) = manifest.get("keywords").and_then(|v| v.as_array()) {
        entry.insert("keywords".into(), Value::Array(kw.clone()));
    }

    let entry_json = Value::Object(entry);
    let source_url = source
        .get("url")
        .or_else(|| source.get("repo"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    sqlx::query!(
        "INSERT INTO plugin_marketplace_entries \
           (tenant_id, plugin_slug, plugin_id, version, source_url, entry_json) \
         VALUES ($1, $2, $3, $4, $5, $6) \
         ON CONFLICT (tenant_id, plugin_slug) DO UPDATE SET \
           plugin_id = EXCLUDED.plugin_id, \
           version = EXCLUDED.version, \
           source_url = EXCLUDED.source_url, \
           entry_json = EXCLUDED.entry_json, \
           updated_at = now()",
        tenant.tenant_id,
        slug,
        plugin_id,
        version,
        source_url,
        entry_json,
    )
    .execute(state.db())
    .await?;
    Ok(())
}

/// Build the `source` JSON sub-object per the Claude Code marketplace spec.
fn build_source(
    sourcing_mode: &str,
    external_git_url: Option<&str>,
    origin: &str,
    slug: &str,
) -> AppResult<Value> {
    match sourcing_mode {
        "internal" | "mirror" => Ok(json!({
            "source": "url",
            "url": format!("{origin}/git/plugins/{slug}.git"),
        })),
        "external" => {
            let url = external_git_url.ok_or_else(|| {
                AppError::BadRequest("external sourcing_mode requires external_git_url".into())
            })?;
            // `github.com/<owner>/<repo>` short-form when applicable.
            if let Some(repo) = parse_github_repo(url) {
                Ok(json!({ "source": "github", "repo": repo }))
            } else {
                Ok(json!({ "source": "url", "url": url }))
            }
        }
        other => Err(AppError::BadRequest(format!(
            "unknown sourcing_mode `{other}`"
        ))),
    }
}

/// Match `https?://github.com/<owner>/<repo>(.git)?` → `<owner>/<repo>`.
fn parse_github_repo(url: &str) -> Option<String> {
    let trimmed = url.trim();
    let body = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))?;
    let body = body.strip_suffix(".git").unwrap_or(body);
    let mut parts = body.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if owner.is_empty() || repo.is_empty() {
        return None;
    }
    if parts.next().is_some() {
        return None; // subpath; only top-level repos qualify for the short form
    }
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn etag_is_stable_for_same_body() {
        let a = compute_etag(b"hello");
        let b = compute_etag(b"hello");
        assert_eq!(a, b);
        // 32 hex chars wrapped in quotes.
        assert_eq!(a.len(), 34);
    }

    #[test]
    fn etag_changes_with_body() {
        assert_ne!(compute_etag(b"a"), compute_etag(b"b"));
    }

    #[test]
    fn origin_uses_x_forwarded_proto_when_present() {
        let mut h = HeaderMap::new();
        h.insert("host", "acme.skill-pool.example.com".parse().unwrap());
        h.insert("x-forwarded-proto", "https".parse().unwrap());
        assert_eq!(
            origin_from_request(&h),
            "https://acme.skill-pool.example.com"
        );
    }

    #[test]
    fn origin_falls_back_to_http_in_tests() {
        let mut h = HeaderMap::new();
        h.insert("host", "acme.localhost:8080".parse().unwrap());
        assert_eq!(origin_from_request(&h), "http://acme.localhost:8080");
    }

    #[test]
    fn build_source_internal_points_at_local_git() {
        let s = build_source("internal", None, "https://acme.example.com", "rust-toolkit").unwrap();
        assert_eq!(s["source"], "url");
        assert_eq!(
            s["url"],
            "https://acme.example.com/git/plugins/rust-toolkit.git"
        );
    }

    #[test]
    fn build_source_mirror_also_points_at_local_git() {
        let s = build_source(
            "mirror",
            Some("https://github.com/acme/foo.git"),
            "https://acme.example.com",
            "foo",
        )
        .unwrap();
        assert_eq!(s["url"], "https://acme.example.com/git/plugins/foo.git");
    }

    #[test]
    fn build_source_external_passes_through() {
        let s = build_source(
            "external",
            Some("https://gitlab.example/acme/plugin.git"),
            "https://acme.example.com",
            "plugin",
        )
        .unwrap();
        assert_eq!(s["source"], "url");
        assert_eq!(s["url"], "https://gitlab.example/acme/plugin.git");
    }

    #[test]
    fn build_source_external_uses_github_short_form() {
        let s = build_source(
            "external",
            Some("https://github.com/acme/plugin"),
            "https://acme.example.com",
            "plugin",
        )
        .unwrap();
        assert_eq!(s["source"], "github");
        assert_eq!(s["repo"], "acme/plugin");
    }

    #[test]
    fn build_source_external_strips_git_suffix() {
        assert_eq!(
            parse_github_repo("https://github.com/acme/plugin.git").as_deref(),
            Some("acme/plugin")
        );
    }

    #[test]
    fn parse_github_repo_rejects_subpaths() {
        assert!(parse_github_repo("https://github.com/acme/plugin/tree/main").is_none());
        assert!(parse_github_repo("https://gitlab.com/acme/plugin").is_none());
    }
}
