//! Theme endpoints.
//!
//! - GET /v1/theme — tenant-scoped, no auth needed (login page renders the
//!   tenant's brand before any user has authenticated).
//! - PUT /v1/theme — requires `tenant:admin` scope.
//! - POST /v1/theme/logo — multipart upload, requires `tenant:admin`. Bytes
//!   are sanitized (`logo_sanitize`) before they ever touch storage.
//! - DELETE /v1/theme/logo — removes the stored logo blob + clears columns.
//! - GET /v1/theme/logo — public (matches `/v1/theme`'s auth model). Streams
//!   the sanitized bytes back with the stored content-type.
//! - POST /v1/theme/favicon — multipart upload, requires `tenant:admin`. Same
//!   sanitizer as the logo plus `image/x-icon`. 64 KiB cap.
//! - DELETE /v1/theme/favicon — clears the favicon blob + columns.
//! - GET /v1/theme/favicon — public. Falls back to the logo bytes when no
//!   favicon is uploaded but a logo is. 404 only when neither exists.
//! - GET /v1/theme/fonts — public; returns the curated font allowlist so the
//!   admin UI can populate the picker without hard-coding the list twice.

use axum::extract::{Multipart, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use bytes::Bytes;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::audit;
use crate::auth::AuthedCaller;
use crate::cache;
use crate::css_sanitize::{self, MAX_CSS_BYTES};
use crate::error::{AppError, AppResult};
use crate::logo_sanitize::{self, LogoKind, MAX_LOGO_BYTES};
use crate::state::AppState;
use crate::storage::Storage;
use crate::tenant::TenantCtx;

/// Redis cache TTL for `GET /v1/theme` payloads, in seconds.
///
/// 5 minutes. Themes are touched by admins, not end users; a longer TTL
/// would mean a fresh "save" looks like it didn't take effect for the
/// duration. We also invalidate explicitly on every mutating endpoint
/// (PUT /v1/theme, logo/favicon/custom-css upload + delete) so the TTL
/// is really a "freshness ceiling for the case where someone updated
/// the tenant_theme row out-of-band" — bytes are dirty for at most
/// `THEME_CACHE_TTL_SECS` seconds.
const THEME_CACHE_TTL_SECS: usize = 300;

fn theme_cache_key(tenant_id: uuid::Uuid) -> String {
    format!("theme:v1:{tenant_id}")
}

/// Invalidate the theme cache for a tenant. Best-effort: errors are
/// logged inside the cache layer, never propagated.
async fn invalidate_theme_cache(state: &AppState, tenant_id: uuid::Uuid) {
    if let Some(redis) = state.redis() {
        let _ = cache::invalidate(redis, &theme_cache_key(tenant_id)).await;
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Theme {
    pub brand_name: String,
    pub primary: String,
    pub primary_fg: String,
    pub accent: String,
    pub bg: String,
    pub fg: String,
    pub muted: String,
    pub muted_fg: String,
    pub border: String,
    pub radius: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
    /// When true the "Powered by skill-pool" footer is shown on every authed
    /// page. Default true (Free tier). Enterprise tenants may turn this off.
    #[serde(default = "default_true")]
    pub footer_branding: bool,
    /// Optional font family selection. `None` (or absent) means inherit the
    /// system stack. Must be a value from `ALLOWED_FONTS` — server-side
    /// validation guards against typos and arbitrary CSS injection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_family: Option<String>,
}

/// Curated allowlist of web-safe, permissively-licenced font families. Every
/// entry below is licenced for both web embedding via Google Fonts AND
/// self-hosting via downloaded files (OFL or Apache 2.0). The list is kept
/// deliberately short — picking one of twelve good fonts is a friendlier
/// experience than scrolling through Google's full 1500+ catalog, and it
/// keeps the loaded-stylesheet weight predictable.
///
/// Rationale per font (line-by-line so anyone adding a new option has to
/// document the choice):
///   - `system` — the OS-native stack. Zero network cost; the safe default.
///   - `Inter` — Rasmus Andersson's neo-grotesque, the de-facto standard for
///     dashboard / SaaS UIs.
///   - `IBM Plex Sans` — IBM's open identity face, excellent for dense data.
///   - `JetBrains Mono` — high-readability monospace for code-heavy UIs.
///   - `Source Sans 3` — Adobe's open sans-serif, very neutral.
///   - `Source Serif 4` — its serif counterpart for editorial layouts.
///   - `Merriweather` — proven body-text serif, optimized for screens.
///   - `Roboto` — Google's flagship sans-serif, near-universal recognition.
///   - `Fira Sans` — Mozilla's open humanist sans, friendly tone.
///   - `Atkinson Hyperlegible` — Braille Institute's accessibility-first face.
///   - `Work Sans` — Wei Huang's open grotesque, optimized for screen.
///   - `Lora` — well-balanced contemporary serif for long-form content.
pub const ALLOWED_FONTS: &[&str] = &[
    "system",
    "Inter",
    "IBM Plex Sans",
    "JetBrains Mono",
    "Source Sans 3",
    "Source Serif 4",
    "Merriweather",
    "Roboto",
    "Fira Sans",
    "Atkinson Hyperlegible",
    "Work Sans",
    "Lora",
];

fn default_true() -> bool {
    true
}

impl Theme {
    fn default_for(slug: &str) -> Self {
        Self {
            brand_name: slug.to_string(),
            primary: "#2563eb".into(),
            primary_fg: "#ffffff".into(),
            accent: "#0ea5e9".into(),
            bg: "#ffffff".into(),
            fg: "#0f172a".into(),
            muted: "#f1f5f9".into(),
            muted_fg: "#475569".into(),
            border: "#e2e8f0".into(),
            radius: "0.5rem".into(),
            logo_uri: None,
            footer_branding: true,
            font_family: None,
        }
    }
}

pub async fn get_theme(State(state): State<AppState>, tenant: TenantCtx) -> AppResult<Json<Theme>> {
    // Hot path: when Redis is available, wrap the DB lookup with a
    // read-through cache keyed by tenant_id. Misses fall through to the
    // exact same SELECT we'd otherwise run; the cache write-back is
    // best-effort inside `cached_json`.
    if let Some(redis) = state.redis() {
        let key = theme_cache_key(tenant.tenant_id);
        let db = state.db().clone();
        let tenant_id = tenant.tenant_id;
        let tenant_slug = tenant.tenant_slug.clone();
        let theme = cache::cached_json(redis, &key, THEME_CACHE_TTL_SECS, move || async move {
            let row = sqlx::query!(
                "SELECT brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, \
                        border, radius, logo_uri, footer_branding, font_family \
                 FROM tenant_theme WHERE tenant_id = $1",
                tenant_id,
            )
            .fetch_optional(&db)
            .await?;
            Ok(match row {
                Some(r) => Theme {
                    brand_name: r.brand_name,
                    primary: r.primary_,
                    primary_fg: r.primary_fg,
                    accent: r.accent,
                    bg: r.bg,
                    fg: r.fg,
                    muted: r.muted,
                    muted_fg: r.muted_fg,
                    border: r.border,
                    radius: r.radius,
                    logo_uri: r.logo_uri,
                    footer_branding: r.footer_branding,
                    font_family: r.font_family,
                },
                None => Theme::default_for(&tenant_slug),
            })
        })
        .await
        .map_err(AppError::Anyhow)?;
        return Ok(Json(theme));
    }

    let row = sqlx::query!(
        "SELECT brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, \
                border, radius, logo_uri, footer_branding, font_family \
         FROM tenant_theme WHERE tenant_id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;

    let theme = match row {
        Some(r) => Theme {
            brand_name: r.brand_name,
            primary: r.primary_,
            primary_fg: r.primary_fg,
            accent: r.accent,
            bg: r.bg,
            fg: r.fg,
            muted: r.muted,
            muted_fg: r.muted_fg,
            border: r.border,
            radius: r.radius,
            logo_uri: r.logo_uri,
            footer_branding: r.footer_branding,
            font_family: r.font_family,
        },
        None => Theme::default_for(&tenant.tenant_slug),
    };
    Ok(Json(theme))
}

/// `GET /v1/theme/fonts` — public; returns the curated allowlist so the admin
/// UI can populate the picker without hard-coding the list a second time.
pub async fn get_fonts() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "allowed": ALLOWED_FONTS }))
}

pub async fn put_theme(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<Theme>,
) -> AppResult<(StatusCode, Json<Theme>)> {
    require_scope(&caller.scope, "tenant:admin")?;
    validate(&body)?;

    sqlx::query!(
        "INSERT INTO tenant_theme \
           (tenant_id, brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, border, radius, logo_uri, footer_branding, font_family) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
           brand_name = EXCLUDED.brand_name, \
           primary_ = EXCLUDED.primary_, \
           primary_fg = EXCLUDED.primary_fg, \
           accent = EXCLUDED.accent, \
           bg = EXCLUDED.bg, \
           fg = EXCLUDED.fg, \
           muted = EXCLUDED.muted, \
           muted_fg = EXCLUDED.muted_fg, \
           border = EXCLUDED.border, \
           radius = EXCLUDED.radius, \
           logo_uri = EXCLUDED.logo_uri, \
           footer_branding = EXCLUDED.footer_branding, \
           font_family = EXCLUDED.font_family",
        caller.tenant.tenant_id,
        &body.brand_name,
        &body.primary,
        &body.primary_fg,
        &body.accent,
        &body.bg,
        &body.fg,
        &body.muted,
        &body.muted_fg,
        &body.border,
        &body.radius,
        body.logo_uri.as_deref(),
        body.footer_branding,
        body.font_family.as_deref(),
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.update",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::to_value(&body).unwrap_or(serde_json::Value::Null),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    Ok((StatusCode::OK, Json(body)))
}

fn require_scope(scope: &str, needed: &str) -> AppResult<()> {
    if scope.split_whitespace().any(|s| s == needed || s == "*") {
        Ok(())
    } else {
        Err(AppError::Forbidden)
    }
}

fn validate(t: &Theme) -> AppResult<()> {
    static HEX: OnceLock<Regex> = OnceLock::new();
    let hex = HEX.get_or_init(|| Regex::new(r"^#[0-9A-Fa-f]{3,8}$").unwrap());

    let colours = [
        ("primary", &t.primary),
        ("primary_fg", &t.primary_fg),
        ("accent", &t.accent),
        ("bg", &t.bg),
        ("fg", &t.fg),
        ("muted", &t.muted),
        ("muted_fg", &t.muted_fg),
        ("border", &t.border),
    ];
    for (field, value) in colours {
        if !hex.is_match(value) {
            return Err(AppError::BadRequest(format!(
                "{field} must be a hex colour like #RRGGBB; got {value:?}"
            )));
        }
    }

    if t.brand_name.is_empty() || t.brand_name.len() > 80 {
        return Err(AppError::BadRequest(
            "brand_name must be 1..=80 characters".into(),
        ));
    }

    // Body-text contrast — WCAG AA = 4.5:1.
    let contrast = contrast_ratio(&t.fg, &t.bg)?;
    if contrast < 4.5 {
        return Err(AppError::BadRequest(format!(
            "body text contrast (fg vs bg) is {contrast:.2}:1; WCAG AA requires 4.5:1"
        )));
    }

    // Font family must be one of the curated allowlist. We do an
    // exact match: "Inter" yes, "inter" or "inter," no. Keeping the
    // comparison strict means a tenant cannot smuggle CSS shenanigans
    // through the column.
    if let Some(font) = t.font_family.as_deref() {
        if !ALLOWED_FONTS.contains(&font) {
            return Err(AppError::BadRequest(format!(
                "font_family {font:?} is not in the allowlist; allowed values: {}",
                ALLOWED_FONTS.join(", ")
            )));
        }
    }

    Ok(())
}

fn contrast_ratio(a: &str, b: &str) -> AppResult<f64> {
    let la = relative_luminance(a)?;
    let lb = relative_luminance(b)?;
    let (lighter, darker) = if la > lb { (la, lb) } else { (lb, la) };
    Ok((lighter + 0.05) / (darker + 0.05))
}

fn relative_luminance(hex: &str) -> AppResult<f64> {
    let (r, g, b) = parse_hex(hex)?;
    let f = |c: u8| {
        let s = c as f64 / 255.0;
        if s <= 0.03928 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    };
    Ok(0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b))
}

fn parse_hex(hex: &str) -> AppResult<(u8, u8, u8)> {
    let s = hex
        .strip_prefix('#')
        .ok_or_else(|| AppError::BadRequest(format!("hex colour must start with #: {hex:?}")))?;
    let (r, g, b) = match s.len() {
        3 => (
            u8::from_str_radix(&s[0..1].repeat(2), 16),
            u8::from_str_radix(&s[1..2].repeat(2), 16),
            u8::from_str_radix(&s[2..3].repeat(2), 16),
        ),
        6 | 8 => (
            u8::from_str_radix(&s[0..2], 16),
            u8::from_str_radix(&s[2..4], 16),
            u8::from_str_radix(&s[4..6], 16),
        ),
        _ => {
            return Err(AppError::BadRequest(format!(
                "hex colour must be 3, 6, or 8 hex digits: {hex:?}"
            )))
        }
    };
    Ok((
        r.map_err(|_| AppError::BadRequest(format!("bad hex: {hex:?}")))?,
        g.map_err(|_| AppError::BadRequest(format!("bad hex: {hex:?}")))?,
        b.map_err(|_| AppError::BadRequest(format!("bad hex: {hex:?}")))?,
    ))
}

// --- logo upload / serve --------------------------------------------------

/// `POST /v1/theme/logo` — multipart upload, single field `file`.
///
/// We deliberately read the `Content-Type` from the multipart part header
/// (not from `Authorization` or any client-controlled state) and feed it to
/// `logo_sanitize::sanitize` along with the raw bytes. The sanitizer is the
/// only thing standing between an admin user and persisted XSS, so it runs
/// **before** anything hits storage.
pub async fn post_logo(
    State(state): State<AppState>,
    caller: AuthedCaller,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<Theme>)> {
    require_scope(&caller.scope, "tenant:admin")?;

    let mut content_type: Option<String> = None;
    let mut bytes: Option<Bytes> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() == Some("file") {
            content_type = field.content_type().map(|s| s.to_string());
            let body = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("file: {e}")))?;
            bytes = Some(body);
        }
    }

    let ct = content_type
        .ok_or_else(|| AppError::BadRequest("multipart `file` part missing Content-Type".into()))?;
    let raw = bytes.ok_or_else(|| AppError::BadRequest("missing `file` field".into()))?;

    if raw.len() > MAX_LOGO_BYTES {
        return Err(AppError::BadRequest(format!(
            "logo too large: {} bytes (max {})",
            raw.len(),
            MAX_LOGO_BYTES
        )));
    }

    let sanitized = logo_sanitize::sanitize(&ct, raw.as_ref())
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // One key per tenant, named by extension so the file is recognisable on
    // disk. If a tenant uploads SVG then later PNG, the SVG blob is left
    // orphaned — clear it out so storage stays tidy.
    let key = Storage::logo_key(caller.tenant.tenant_id, sanitized.kind.extension());
    let storage = state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?;

    // Best-effort cleanup of any previously-stored logo for this tenant
    // (different extension → different key). Failing to delete an orphan
    // shouldn't block the upload.
    let prev = sqlx::query!(
        "SELECT logo_storage_key FROM tenant_theme WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;
    if let Some(r) = prev {
        if let Some(prev_key) = r.logo_storage_key {
            if prev_key != key {
                let _ = storage.delete_object(&prev_key).await;
            }
        }
    }

    storage
        .put_object(&key, Bytes::from(sanitized.bytes.clone()))
        .await
        .map_err(AppError::Anyhow)?;

    let size: i32 = sanitized.bytes.len() as i32;
    sqlx::query!(
        "INSERT INTO tenant_theme (tenant_id, brand_name, logo_storage_key, logo_content_type, logo_bytes_size) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            logo_storage_key  = EXCLUDED.logo_storage_key, \
            logo_content_type = EXCLUDED.logo_content_type, \
            logo_bytes_size   = EXCLUDED.logo_bytes_size",
        caller.tenant.tenant_id,
        &caller.tenant.tenant_slug,
        &key,
        sanitized.kind.content_type(),
        size,
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.logo.upload",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::json!({
                "content_type": sanitized.kind.content_type(),
                "size_bytes": size,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    // Return the freshly-updated theme so the UI can re-render.
    let theme = read_theme(state.db(), &caller.tenant).await?;
    Ok((StatusCode::OK, Json(theme)))
}

/// `DELETE /v1/theme/logo` — clear the stored logo + delete the storage
/// object. 204 on success (mirrors common REST conventions). Returns 200 +
/// empty JSON if no logo was set, since the column was already null.
pub async fn delete_logo(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "tenant:admin")?;

    let prev = sqlx::query!(
        "SELECT logo_storage_key FROM tenant_theme WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;

    if let Some(r) = prev {
        if let Some(key) = r.logo_storage_key {
            let storage = state
                .storage_for(&caller.tenant)
                .await
                .map_err(AppError::Anyhow)?;
            storage
                .delete_object(&key)
                .await
                .map_err(AppError::Anyhow)?;
        }
    }

    sqlx::query!(
        "UPDATE tenant_theme \
            SET logo_storage_key = NULL, logo_content_type = NULL, logo_bytes_size = NULL \
          WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.logo.delete",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::Value::Null,
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/theme/logo` — public; serves the stored bytes with the matching
/// content-type. 404 when no upload exists (caller should fall back to the
/// `logo_uri` field of `GET /v1/theme`).
///
/// Cache headers: `Cache-Control: public, max-age=300`. Five minutes is the
/// sweet spot — long enough to dodge load on the login page, short enough
/// that a logo replace is visible across the org within minutes.
pub async fn get_logo(State(state): State<AppState>, tenant: TenantCtx) -> AppResult<Response> {
    let row = sqlx::query!(
        "SELECT logo_storage_key, logo_content_type FROM tenant_theme WHERE tenant_id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db_read())
    .await?;

    let (key, ct) = match row {
        Some(r) => match (r.logo_storage_key, r.logo_content_type) {
            (Some(k), Some(c)) => (k, c),
            _ => return Err(AppError::NotFound),
        },
        None => return Err(AppError::NotFound),
    };

    let storage = state.storage_for(&tenant).await.map_err(AppError::Anyhow)?;
    let bytes = storage
        .read_object(&key)
        .await
        .map_err(AppError::Anyhow)?
        .ok_or(AppError::NotFound)?;

    let mut resp = (StatusCode::OK, bytes).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&ct) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    Ok(resp)
}

// --- favicon upload / serve ----------------------------------------------

/// 64 KiB. Favicons should be tiny; a smaller cap than the logo's 256 KiB
/// nudges admins toward sensibly-sized assets and matches the DB CHECK in
/// migration 0023.
pub const MAX_FAVICON_BYTES: usize = 64 * 1024;

/// `POST /v1/theme/favicon` — multipart upload, single field `file`.
///
/// Mirrors `post_logo` with three deltas:
///   1. 64 KiB cap rather than 256 KiB.
///   2. `image/x-icon` is accepted in addition to the four logo formats.
///   3. The sanitizer ICO branch only does a magic-byte check — ICO is
///      not script-bearing, so a structural validate is enough.
pub async fn post_favicon(
    State(state): State<AppState>,
    caller: AuthedCaller,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<Theme>)> {
    require_scope(&caller.scope, "tenant:admin")?;

    let mut content_type: Option<String> = None;
    let mut bytes: Option<Bytes> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() == Some("file") {
            content_type = field.content_type().map(|s| s.to_string());
            let body = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("file: {e}")))?;
            bytes = Some(body);
        }
    }

    let ct = content_type
        .ok_or_else(|| AppError::BadRequest("multipart `file` part missing Content-Type".into()))?;
    let raw = bytes.ok_or_else(|| AppError::BadRequest("missing `file` field".into()))?;

    if raw.len() > MAX_FAVICON_BYTES {
        return Err(AppError::BadRequest(format!(
            "favicon too large: {} bytes (max {})",
            raw.len(),
            MAX_FAVICON_BYTES
        )));
    }

    let sanitized = logo_sanitize::sanitize(&ct, raw.as_ref())
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    let key = Storage::favicon_key(caller.tenant.tenant_id, sanitized.kind.extension());
    let storage = state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?;

    // Best-effort cleanup of any previously-stored favicon under a different
    // extension. Identical pattern to logo upload.
    let prev = sqlx::query!(
        "SELECT favicon_storage_key FROM tenant_theme WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;
    if let Some(r) = prev {
        if let Some(prev_key) = r.favicon_storage_key {
            if prev_key != key {
                let _ = storage.delete_object(&prev_key).await;
            }
        }
    }

    storage
        .put_object(&key, Bytes::from(sanitized.bytes.clone()))
        .await
        .map_err(AppError::Anyhow)?;

    let size: i32 = sanitized.bytes.len() as i32;
    sqlx::query!(
        "INSERT INTO tenant_theme (tenant_id, brand_name, favicon_storage_key, favicon_content_type, favicon_bytes_size) \
         VALUES ($1, $2, $3, $4, $5) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            favicon_storage_key  = EXCLUDED.favicon_storage_key, \
            favicon_content_type = EXCLUDED.favicon_content_type, \
            favicon_bytes_size   = EXCLUDED.favicon_bytes_size",
        caller.tenant.tenant_id,
        &caller.tenant.tenant_slug,
        &key,
        sanitized.kind.content_type(),
        size,
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.favicon.upload",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::json!({
                "content_type": sanitized.kind.content_type(),
                "size_bytes": size,
            }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    let theme = read_theme(state.db(), &caller.tenant).await?;
    Ok((StatusCode::OK, Json(theme)))
}

/// `DELETE /v1/theme/favicon` — clear the stored favicon + remove the blob.
pub async fn delete_favicon(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "tenant:admin")?;

    let prev = sqlx::query!(
        "SELECT favicon_storage_key FROM tenant_theme WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;

    if let Some(r) = prev {
        if let Some(key) = r.favicon_storage_key {
            let storage = state
                .storage_for(&caller.tenant)
                .await
                .map_err(AppError::Anyhow)?;
            storage
                .delete_object(&key)
                .await
                .map_err(AppError::Anyhow)?;
        }
    }

    sqlx::query!(
        "UPDATE tenant_theme \
            SET favicon_storage_key = NULL, favicon_content_type = NULL, favicon_bytes_size = NULL \
          WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.favicon.delete",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::Value::Null,
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/theme/favicon` — public; serves bytes. If no favicon row, falls
/// back to the logo bytes. 404 only when neither exists. Same 5-minute cache.
///
/// The fallback is intentional: every browser hits `/favicon.ico` (or the
/// `<link rel="icon">` target) regardless of whether the tenant has uploaded
/// one. Serving the logo at favicon size is a sensible default — browsers
/// scale SVG/PNG to whatever box the chrome needs, and the logo is the
/// closest thing we have to "the brand mark".
pub async fn get_favicon(State(state): State<AppState>, tenant: TenantCtx) -> AppResult<Response> {
    // Try favicon first; fall back to logo when favicon row is empty.
    let row = sqlx::query!(
        "SELECT favicon_storage_key, favicon_content_type, logo_storage_key, logo_content_type \
         FROM tenant_theme WHERE tenant_id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db_read())
    .await?;

    let (key, ct) = match row {
        Some(ref r) if r.favicon_storage_key.is_some() && r.favicon_content_type.is_some() => (
            r.favicon_storage_key.clone().unwrap(),
            r.favicon_content_type.clone().unwrap(),
        ),
        Some(ref r)
            if r.favicon_storage_key.is_none()
                && r.logo_storage_key.is_some()
                && r.logo_content_type.is_some() =>
        {
            (
                r.logo_storage_key.clone().unwrap(),
                r.logo_content_type.clone().unwrap(),
            )
        }
        _ => return Err(AppError::NotFound),
    };

    let storage = state.storage_for(&tenant).await.map_err(AppError::Anyhow)?;
    let bytes = storage
        .read_object(&key)
        .await
        .map_err(AppError::Anyhow)?
        .ok_or(AppError::NotFound)?;

    let mut resp = (StatusCode::OK, bytes).into_response();
    let headers = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&ct) {
        headers.insert(header::CONTENT_TYPE, v);
    }
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    Ok(resp)
}

/// Helper for `post_logo` to re-fetch the theme after writing. Mirrors
/// `get_theme` but takes a `TenantCtx` directly so we don't have to thread
/// the extractor through.
async fn read_theme(db: &sqlx::PgPool, tenant: &TenantCtx) -> AppResult<Theme> {
    let row = sqlx::query!(
        "SELECT brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, \
                border, radius, logo_uri, footer_branding, font_family \
         FROM tenant_theme WHERE tenant_id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(match row {
        Some(r) => Theme {
            brand_name: r.brand_name,
            primary: r.primary_,
            primary_fg: r.primary_fg,
            accent: r.accent,
            bg: r.bg,
            fg: r.fg,
            muted: r.muted,
            muted_fg: r.muted_fg,
            border: r.border,
            radius: r.radius,
            logo_uri: r.logo_uri,
            footer_branding: r.footer_branding,
            font_family: r.font_family,
        },
        None => Theme::default_for(&tenant.tenant_slug),
    })
}

// Tiny compile-time assurance that `LogoKind`'s content-type matches the
// values our DB CHECK constraint accepts. If anyone adds a new variant we
// want a visible failure here rather than a runtime constraint violation.
const _: () = {
    // Force usage of LogoKind in a `match` so a new variant is a build error.
    fn _exhaustive(k: LogoKind) -> &'static str {
        match k {
            LogoKind::Svg => "image/svg+xml",
            LogoKind::Png => "image/png",
            LogoKind::Jpeg => "image/jpeg",
            LogoKind::Webp => "image/webp",
            LogoKind::Ico => "image/x-icon",
        }
    }
    let _ = _exhaustive;
};

// --- custom CSS upload / serve --------------------------------------------

/// `POST /v1/theme/custom-css` — multipart upload, single field `file`.
///
/// The handler is structurally identical to `post_logo`: read the bytes from
/// multipart, run the sanitizer (which is the only thing standing between an
/// admin token and persisted CSS injection), persist via `storage_for`, and
/// update the `tenant_theme` row.
///
/// The sanitizer rejects `@import`, `url()` pointing off-site, `expression()`,
/// `behavior:`, `javascript:` URIs, HTML-tag-like bytes, and the literal
/// `</style>` sequence — see `css_sanitize.rs` for the full list. The GET
/// endpoint additionally pins `Content-Security-Policy: style-src 'self'`
/// as defence in depth.
pub async fn post_custom_css(
    State(state): State<AppState>,
    caller: AuthedCaller,
    mut multipart: Multipart,
) -> AppResult<(StatusCode, Json<Theme>)> {
    require_scope(&caller.scope, "tenant:admin")?;

    let mut bytes: Option<Bytes> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| AppError::BadRequest(format!("multipart: {e}")))?
    {
        if field.name() == Some("file") {
            let body = field
                .bytes()
                .await
                .map_err(|e| AppError::BadRequest(format!("file: {e}")))?;
            bytes = Some(body);
        }
    }

    let raw = bytes.ok_or_else(|| AppError::BadRequest("missing `file` field".into()))?;

    if raw.len() > MAX_CSS_BYTES {
        return Err(AppError::BadRequest(format!(
            "custom CSS too large: {} bytes (max {})",
            raw.len(),
            MAX_CSS_BYTES
        )));
    }

    let sanitized =
        css_sanitize::sanitize(raw.as_ref()).map_err(|e| AppError::BadRequest(e.to_string()))?;

    let key = Storage::custom_css_key(caller.tenant.tenant_id);
    let storage = state
        .storage_for(&caller.tenant)
        .await
        .map_err(AppError::Anyhow)?;

    storage
        .put_object(&key, Bytes::from(sanitized.bytes.clone()))
        .await
        .map_err(AppError::Anyhow)?;

    let size: i32 = sanitized.bytes.len() as i32;
    sqlx::query!(
        "INSERT INTO tenant_theme (tenant_id, brand_name, custom_css_storage_key, custom_css_bytes_size) \
         VALUES ($1, $2, $3, $4) \
         ON CONFLICT (tenant_id) DO UPDATE SET \
            custom_css_storage_key = EXCLUDED.custom_css_storage_key, \
            custom_css_bytes_size  = EXCLUDED.custom_css_bytes_size",
        caller.tenant.tenant_id,
        &caller.tenant.tenant_slug,
        &key,
        size,
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.custom_css.upload",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::json!({ "size_bytes": size }),
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    let theme = read_theme(state.db(), &caller.tenant).await?;
    Ok((StatusCode::OK, Json(theme)))
}

/// `DELETE /v1/theme/custom-css` — clear the stored overlay + delete the
/// storage object. 204 on success.
pub async fn delete_custom_css(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<StatusCode> {
    require_scope(&caller.scope, "tenant:admin")?;

    let prev = sqlx::query!(
        "SELECT custom_css_storage_key FROM tenant_theme WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .fetch_optional(state.db())
    .await?;

    if let Some(r) = prev {
        if let Some(key) = r.custom_css_storage_key {
            let storage = state
                .storage_for(&caller.tenant)
                .await
                .map_err(AppError::Anyhow)?;
            storage
                .delete_object(&key)
                .await
                .map_err(AppError::Anyhow)?;
        }
    }

    sqlx::query!(
        "UPDATE tenant_theme \
            SET custom_css_storage_key = NULL, custom_css_bytes_size = NULL \
          WHERE tenant_id = $1",
        caller.tenant.tenant_id,
    )
    .execute(state.db())
    .await?;

    audit::record_best_effort(
        state.db(),
        audit::Event {
            tenant_id: caller.tenant.tenant_id,
            actor_user: None,
            actor_token: Some(caller.token_id),
            action: "theme.custom_css.delete",
            target_kind: "theme",
            target_id: None,
            metadata: serde_json::Value::Null,
            ip_addr: None,
            user_agent: None,
        },
    )
    .await;

    invalidate_theme_cache(&state, caller.tenant.tenant_id).await;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /v1/theme/custom.css` — public; serves the sanitized bytes as
/// `text/css; charset=utf-8`. 404 when no overlay is set.
///
/// Headers:
///   * `Content-Type: text/css; charset=utf-8` — pinned so the response
///     cannot be reinterpreted as HTML by browsers that sniff.
///   * `Cache-Control: public, max-age=300` — five minutes mirrors the
///     logo / favicon endpoints; a fresh upload propagates within that
///     window.
///   * `Content-Security-Policy: style-src 'self'` — defence in depth.
///     Even if a `url(https://evil.com/x.css)` slipped past the sanitizer,
///     the response itself is forbidden from loading external sheets when
///     rendered under its own CSP. The parent document still controls the
///     CSP that applies to its own `<link rel="stylesheet">` tags; this
///     header pins the response-level policy that browsers honour when the
///     resource is fetched standalone or in worker contexts.
///   * `X-Content-Type-Options: nosniff` — close the "mis-typed as HTML"
///     escape hatch on older browsers.
pub async fn get_custom_css(
    State(state): State<AppState>,
    tenant: TenantCtx,
) -> AppResult<Response> {
    let row = sqlx::query!(
        "SELECT custom_css_storage_key FROM tenant_theme WHERE tenant_id = $1",
        tenant.tenant_id,
    )
    .fetch_optional(state.db_read())
    .await?;

    let key = match row.and_then(|r| r.custom_css_storage_key) {
        Some(k) => k,
        None => return Err(AppError::NotFound),
    };

    let storage = state.storage_for(&tenant).await.map_err(AppError::Anyhow)?;
    let bytes = storage
        .read_object(&key)
        .await
        .map_err(AppError::Anyhow)?
        .ok_or(AppError::NotFound)?;

    let mut resp = (StatusCode::OK, bytes).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/css; charset=utf-8"),
    );
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=300"),
    );
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("style-src 'self'"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    Ok(resp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contrast_of_black_on_white_is_21() {
        let c = contrast_ratio("#000000", "#ffffff").unwrap();
        assert!((c - 21.0).abs() < 0.01, "got {c}");
    }

    #[test]
    fn validates_default_theme() {
        let t = Theme::default_for("acme");
        validate(&t).unwrap();
    }

    #[test]
    fn rejects_low_contrast() {
        let mut t = Theme::default_for("acme");
        t.bg = "#fff8f0".into();
        t.fg = "#fefefe".into(); // basically invisible
        let err = validate(&t).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn rejects_bad_hex() {
        let mut t = Theme::default_for("acme");
        t.primary = "blue".into();
        let err = validate(&t).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn accepts_allowed_font() {
        let mut t = Theme::default_for("acme");
        t.font_family = Some("Inter".into());
        validate(&t).unwrap();
    }

    #[test]
    fn accepts_no_font() {
        let mut t = Theme::default_for("acme");
        t.font_family = None;
        validate(&t).unwrap();
    }

    #[test]
    fn rejects_unknown_font() {
        let mut t = Theme::default_for("acme");
        t.font_family = Some("Comic Sans MS".into());
        let err = validate(&t).unwrap_err();
        match err {
            AppError::BadRequest(msg) => {
                assert!(msg.contains("Comic Sans MS"), "msg = {msg}");
                assert!(msg.to_lowercase().contains("allowlist"), "msg = {msg}");
            }
            other => panic!("expected BadRequest, got {other:?}"),
        }
    }

    #[test]
    fn allowlist_has_twelve_entries() {
        // Surface a hard test failure if anyone tweaks the list without
        // updating the docs.
        assert_eq!(ALLOWED_FONTS.len(), 12, "ALLOWED_FONTS = {ALLOWED_FONTS:?}");
        assert_eq!(ALLOWED_FONTS[0], "system");
    }
}
