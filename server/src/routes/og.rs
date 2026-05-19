//! Open Graph image generator (#9 / Enterprise).
//!
//! `GET /v1/og?slug=<slug>&kind=<kind>` returns an SVG social-card image
//! sized 1200x630, branded with the tenant's theme. No auth — same model
//! as `/v1/theme` and `/v1/theme/logo` — but tenant-scoped via the usual
//! Host / `X-Skill-Pool-Tenant` resolution.
//!
//! # Why SVG and not PNG?
//!
//! We deliberately chose the lightweight path (zero new build deps).
//! Slack and Discord both render `og:image` SVG previews correctly,
//! including embedded `<image>` data URLs for raster logos. Twitter/X and
//! LinkedIn require PNG/JPG and will show a broken thumbnail when handed
//! an SVG — see `docs/enterprise/og-images.md` for the full breakdown.
//!
//! # Caching
//!
//! The bytes are tiny (10–30 KiB) so we render fresh per request and let
//! HTTP caching do the work. `Cache-Control: public, max-age=86400`
//! (1 day) plus a strong `ETag` derived from the inputs that change the
//! visual output: tenant id, slug, kind, skill version, and the theme's
//! `updated_at`. A theme edit or skill re-publish bumps the ETag, so
//! intermediate caches revalidate cleanly. Social-platform crawlers
//! cache for 7–14 days regardless — that's a platform behaviour, not
//! ours to fix.

use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

/// `GET /v1/og?slug=<slug>&kind=<kind>` query string.
#[derive(Deserialize)]
pub struct OgQuery {
    /// Skill / agent / command slug. Required — a missing slug is a 400
    /// rather than a "tenant default" image, because the social platform
    /// is asking for *this* card, not a generic one.
    pub slug: Option<String>,
    /// Catalog-item kind. Defaults to `skill` to match the rest of the
    /// API surface. `agent` and `command` are accepted; anything else is
    /// a 400 (mirroring `resolve_kind` in `routes::skills`).
    pub kind: Option<String>,
}

/// Pixel canvas. The 1200x630 size is the de-facto Open Graph default
/// (Facebook docs recommend 1200x630 minimum, 1.91:1 aspect ratio).
const OG_WIDTH: u32 = 1200;
const OG_HEIGHT: u32 = 630;

/// Visual budget for description wrapping. Tuned to fit roughly 4 lines
/// at the 28px font size we render at — adjust both together if you
/// ever bump the description font.
const DESCRIPTION_WRAP_CHARS: usize = 70;
const DESCRIPTION_MAX_LINES: usize = 4;

/// One row out of `tenant_theme` — only the columns the OG renderer
/// actually consumes, plus `updated_at` for ETag composition.
#[derive(sqlx::FromRow)]
struct ThemeForOg {
    brand_name: String,
    #[sqlx(rename = "primary_")]
    primary: String,
    primary_fg: String,
    bg: String,
    fg: String,
    muted_fg: String,
    border: String,
    logo_storage_key: Option<String>,
    logo_content_type: Option<String>,
    updated_at: DateTime<Utc>,
}

/// One row out of `skills` — minimum fields needed to render. We only
/// consider published rows, ordered to the latest version.
#[derive(sqlx::FromRow)]
struct SkillForOg {
    version: String,
    description: String,
}

pub async fn og_image(
    State(state): State<AppState>,
    tenant: TenantCtx,
    headers: HeaderMap,
    Query(q): Query<OgQuery>,
) -> AppResult<Response> {
    let slug = q
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| AppError::BadRequest("missing `slug` query parameter".into()))?;
    let kind = resolve_kind(q.kind.as_deref())?;

    // Theme: defaulted when the tenant hasn't customised yet — same
    // shape as `theme::get_theme` so an empty-theme tenant still gets a
    // pleasant card.
    let theme: Option<ThemeForOg> = sqlx::query_as(
        "SELECT brand_name, primary_, primary_fg, bg, fg, muted_fg, border, \
                logo_storage_key, logo_content_type, updated_at \
         FROM tenant_theme WHERE tenant_id = $1",
    )
    .bind(tenant.tenant_id)
    .fetch_optional(state.db_read())
    .await?;
    let theme = theme.unwrap_or_else(|| default_theme_for(&tenant.tenant_slug));

    // Skill: latest published version. A missing slug is 404 — the
    // social crawler will fall back to whatever sibling `og:image` we
    // ship elsewhere, or none. We considered serving a "skill not
    // found" fallback card but decided against it because crawlers
    // cache 200s aggressively; a 404 lets the page-level `og:image`
    // disappear cleanly if the skill is later deleted.
    let skill: Option<SkillForOg> = sqlx::query_as(
        "SELECT version, description \
         FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(tenant.tenant_id)
    .bind(slug)
    .bind(kind)
    .fetch_optional(state.db_read())
    .await?;
    let skill = skill.ok_or(AppError::NotFound)?;

    // ETag composition. The inputs are everything that changes the
    // rendered SVG: tenant id, slug, kind, version (skill re-publish),
    // and theme.updated_at (brand-name / colour / logo edits). We don't
    // hash the description directly — the description is part of the
    // skill row, and a description edit ships a new version (skill
    // publish is immutable per version), so the version covers it.
    let etag_input = format!(
        "{}:{}:{}:{}:{}",
        tenant.tenant_id,
        slug,
        kind,
        skill.version,
        theme.updated_at.timestamp(),
    );
    let mut hasher = Sha256::new();
    hasher.update(etag_input.as_bytes());
    let digest = hasher.finalize();
    let etag_value = format!("\"og-{}\"", hex::encode(&digest[..8]));

    // If-None-Match handling. Strong-tag equality only (we never emit a
    // weak prefix). A list of comma-separated tags is allowed per RFC
    // 9110 §13.1.2; trim and split for completeness.
    if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
        if let Ok(s) = inm.to_str() {
            if s.split(',').map(str::trim).any(|t| t == etag_value) {
                let mut resp = StatusCode::NOT_MODIFIED.into_response();
                let h = resp.headers_mut();
                h.insert(header::ETAG, HeaderValue::from_str(&etag_value).unwrap());
                h.insert(
                    header::CACHE_CONTROL,
                    HeaderValue::from_static("public, max-age=86400"),
                );
                return Ok(resp);
            }
        }
    }

    // Optional logo embed. We pull the bytes once and base64-encode
    // them as a data URI inside the SVG `<image>` element. Browsers
    // (and Slack/Discord previews) render SVG with embedded data URIs
    // fine. Raster logos work too because the outer element is SVG —
    // the *outer* format is what platforms care about.
    //
    // Failure modes (storage miss, unknown content-type) silently fall
    // back to a textual brand initial. We deliberately don't 5xx — a
    // broken share preview is worse than a slightly-less-branded one.
    let logo_data_uri = match (theme.logo_storage_key.as_deref(), theme.logo_content_type.as_deref())
    {
        (Some(key), Some(ct)) => fetch_logo_data_uri(&state, &tenant, key, ct).await,
        _ => None,
    };

    let svg = render_svg(SvgInputs {
        brand_name: &theme.brand_name,
        primary: &theme.primary,
        primary_fg: &theme.primary_fg,
        bg: &theme.bg,
        fg: &theme.fg,
        muted_fg: &theme.muted_fg,
        border: &theme.border,
        skill_slug: slug,
        skill_version: &skill.version,
        skill_description: &skill.description,
        kind,
        logo_data_uri: logo_data_uri.as_deref(),
    });

    let mut resp = (StatusCode::OK, svg).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("image/svg+xml; charset=utf-8"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=86400"),
    );
    h.insert(header::ETAG, HeaderValue::from_str(&etag_value).unwrap());
    Ok(resp)
}

/// Read the stored logo bytes and wrap them in a `data:` URI so the SVG
/// renderer can inline them. Returns `None` on any failure; callers fall
/// back to a textual brand mark.
async fn fetch_logo_data_uri(
    state: &AppState,
    tenant: &TenantCtx,
    key: &str,
    content_type: &str,
) -> Option<String> {
    let storage = state.storage_for(tenant).await.ok()?;
    // `read_object` returns `Result<Option<Bytes>>`; both an outer `Err`
    // and an inner `None` mean "no logo for the OG card today".
    let bytes = storage.read_object(key).await.ok().flatten()?;
    // Cap the logo at 256 KiB before base64 — the sanitizer already
    // enforces this, but being defensive keeps a misconfigured storage
    // backend from blowing the response size.
    if bytes.len() > 256 * 1024 {
        return None;
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Some(format!("data:{};base64,{}", content_type, b64))
}

/// Default theme used when a tenant hasn't called PUT /v1/theme yet.
/// Mirrors `theme::Theme::default_for` (private over there) so the OG
/// card looks identical to a brand-new tenant's UI.
fn default_theme_for(slug: &str) -> ThemeForOg {
    ThemeForOg {
        brand_name: slug.to_string(),
        primary: "#2563eb".into(),
        primary_fg: "#ffffff".into(),
        bg: "#ffffff".into(),
        fg: "#0f172a".into(),
        muted_fg: "#475569".into(),
        border: "#e2e8f0".into(),
        logo_storage_key: None,
        logo_content_type: None,
        // Fixed epoch is fine — a never-customised tenant has a stable
        // ETag until they call PUT /v1/theme (which inserts a real row).
        updated_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap_or_else(Utc::now),
    }
}

/// Catalog kinds we accept. Mirror of `routes::skills::resolve_kind`
/// kept local so we don't have to expose that one. Adding a new kind
/// over there means touching this list too — covered by the test
/// `og_image_rejects_unknown_kind`.
fn resolve_kind(raw: Option<&str>) -> AppResult<&'static str> {
    match raw.unwrap_or("skill").trim() {
        "skill" => Ok("skill"),
        "agent" => Ok("agent"),
        "command" => Ok("command"),
        other => Err(AppError::BadRequest(format!(
            "kind must be one of [\"skill\", \"agent\", \"command\"], got `{other}`"
        ))),
    }
}

// --- SVG rendering --------------------------------------------------------

struct SvgInputs<'a> {
    brand_name: &'a str,
    primary: &'a str,
    primary_fg: &'a str,
    bg: &'a str,
    fg: &'a str,
    muted_fg: &'a str,
    border: &'a str,
    skill_slug: &'a str,
    skill_version: &'a str,
    skill_description: &'a str,
    kind: &'a str,
    logo_data_uri: Option<&'a str>,
}

fn render_svg(i: SvgInputs<'_>) -> String {
    // Geometry. Pulled out as named constants so the layout is readable
    // at a glance — there's no live designer for this, so the SVG
    // *source* is the design system.
    let border_w = 24_u32;
    let pad_x = 80_u32;
    let pad_y = 64_u32;

    // Header row (logo + brand name).
    let logo_size = 56_u32; // a bit bigger than the spec's 40px so it reads at thumbnail scale
    let logo_x = pad_x;
    let logo_y = pad_y;
    let brand_text_x = logo_x + logo_size + 24; // gap between logo and text
    let brand_text_y = logo_y + (logo_size / 2) + 10; // optical centre on the logo
    let brand_font_size = 32_u32;

    // Skill name (big headline).
    let title_x = pad_x;
    let title_y = pad_y + logo_size + 80;
    let title_font_size = 64_u32;

    // Description block.
    let desc_x = pad_x;
    let desc_y = title_y + 64;
    let desc_font_size = 28_u32;
    let desc_line_height = (desc_font_size as f64 * 1.35).round() as u32;

    // Bottom-left "kind:" label.
    let kind_label_x = pad_x;
    let kind_label_y = OG_HEIGHT - pad_y;
    let kind_font_size = 22_u32;

    // Bottom-right version pill.
    let pill_h = 56_u32;
    let pill_pad_x = 28_u32;
    let pill_text = format!("v{}", i.skill_version);
    let pill_text_size = 28_u32;
    let pill_text_width = estimate_text_width(&pill_text, pill_text_size);
    let pill_w = pill_text_width + pill_pad_x * 2;
    let pill_x = OG_WIDTH - pad_x - pill_w;
    let pill_y = OG_HEIGHT - pad_y - pill_h;
    let pill_text_x = pill_x + pill_w / 2;
    let pill_text_y = pill_y + pill_h / 2 + (pill_text_size as i32 / 3) as u32;

    // Wrap the description into lines.
    let description_lines = wrap_description(
        i.skill_description,
        DESCRIPTION_WRAP_CHARS,
        DESCRIPTION_MAX_LINES,
    );

    // Build description tspans. SVG `<text>` doesn't word-wrap on its
    // own; each line gets its own `<tspan>` with an explicit `dy`.
    let mut desc_tspans = String::new();
    for (idx, line) in description_lines.iter().enumerate() {
        let dy = if idx == 0 { 0 } else { desc_line_height };
        desc_tspans.push_str(&format!(
            "<tspan x=\"{x}\" dy=\"{dy}\">{text}</tspan>",
            x = desc_x,
            dy = dy,
            text = escape_xml(line)
        ));
    }

    // Logo element: either an embedded image, or a colored circle with
    // the brand initial. Plain SVG `<image>` with `href=` supports data
    // URIs; the older `xlink:href` is also fine, but vanilla `href`
    // works in SVG 2 + every modern renderer.
    let logo_block = match i.logo_data_uri {
        Some(uri) => format!(
            "<image x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" \
                preserveAspectRatio=\"xMidYMid meet\" href=\"{uri}\"/>",
            x = logo_x,
            y = logo_y,
            w = logo_size,
            h = logo_size,
            uri = escape_attr(uri),
        ),
        None => {
            let initial = i
                .brand_name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "?".into());
            let cx = logo_x + logo_size / 2;
            let cy = logo_y + logo_size / 2;
            let initial_y = cy + 12; // optical baseline
            format!(
                "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"{fill}\"/>\
                 <text x=\"{cx}\" y=\"{ty}\" text-anchor=\"middle\" \
                       font-family=\"system-ui, sans-serif\" \
                       font-size=\"32\" font-weight=\"700\" fill=\"{tfill}\">{initial}</text>",
                cx = cx,
                cy = cy,
                r = logo_size / 2,
                fill = escape_attr(i.primary),
                ty = initial_y,
                tfill = escape_attr(i.primary_fg),
                initial = escape_xml(&initial),
            )
        }
    };

    // Inner-border rectangle. We inset the border by half its stroke
    // width so the visible inner edge lines up with the canvas edge —
    // SVG strokes paint centred on the path.
    let inner_inset = border_w / 2;

    format!(
        r##"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{aria}">
  <rect x="0" y="0" width="{w}" height="{h}" fill="{bg}"/>
  <rect x="{inset}" y="{inset}" width="{rw}" height="{rh}" fill="none" stroke="{primary}" stroke-width="{border_w}"/>
  <g font-family="system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif">
    {logo_block}
    <text x="{bx}" y="{by}" font-size="{brand_size}" font-weight="500" fill="{fg}">{brand_name}</text>
    <text x="{tx}" y="{ty}" font-size="{title_size}" font-weight="700" fill="{fg}">{title}</text>
    <text x="{dx}" y="{dy}" font-size="{desc_size}" fill="{muted_fg}">{desc_tspans}</text>
    <text x="{kx}" y="{ky}" font-size="{kind_size}" font-weight="500" fill="{muted_fg}">kind: {kind}</text>
    <rect x="{px}" y="{py}" width="{pw}" height="{ph}" rx="{prx}" fill="{primary}"/>
    <text x="{ptx}" y="{pty}" text-anchor="middle" font-size="{pill_size}" font-weight="600" fill="{primary_fg}">{pill_text}</text>
  </g>
  <!-- inner border line so the card pops on white background previews -->
  <rect x="0.5" y="0.5" width="{w_minus}" height="{h_minus}" fill="none" stroke="{border}" stroke-width="1"/>
</svg>
"##,
        w = OG_WIDTH,
        h = OG_HEIGHT,
        w_minus = OG_WIDTH - 1,
        h_minus = OG_HEIGHT - 1,
        rw = OG_WIDTH - border_w,
        rh = OG_HEIGHT - border_w,
        inset = inner_inset,
        border_w = border_w,
        bg = escape_attr(i.bg),
        fg = escape_attr(i.fg),
        muted_fg = escape_attr(i.muted_fg),
        primary = escape_attr(i.primary),
        primary_fg = escape_attr(i.primary_fg),
        border = escape_attr(i.border),
        logo_block = logo_block,
        bx = brand_text_x,
        by = brand_text_y,
        brand_size = brand_font_size,
        brand_name = escape_xml(i.brand_name),
        tx = title_x,
        ty = title_y,
        title_size = title_font_size,
        title = escape_xml(i.skill_slug),
        dx = desc_x,
        dy = desc_y,
        desc_size = desc_font_size,
        desc_tspans = desc_tspans,
        kx = kind_label_x,
        ky = kind_label_y,
        kind_size = kind_font_size,
        kind = escape_xml(i.kind),
        px = pill_x,
        py = pill_y,
        pw = pill_w,
        ph = pill_h,
        prx = pill_h / 2,
        ptx = pill_text_x,
        pty = pill_text_y,
        pill_size = pill_text_size,
        pill_text = escape_xml(&pill_text),
        aria = escape_attr(&format!("Open Graph card for {} on {}", i.skill_slug, i.brand_name)),
    )
}

/// Rough monospace-ish width estimate for sizing the version pill. Real
/// text metrics need a font rasterizer; this is good enough to keep the
/// pill from clipping a long version string like `v10.20.30-rc.1`.
///
/// 0.6 of the font size per character matches the average glyph width
/// of the system sans-serif at our sizes within ~10%.
fn estimate_text_width(text: &str, font_size: u32) -> u32 {
    ((text.chars().count() as f64) * (font_size as f64) * 0.6).round() as u32
}

/// Greedy word-wrap. Splits on whitespace, packs as many words as fit
/// under `max_chars` per line, truncates with an ellipsis on the final
/// line if there's overflow. Pure ASCII char counting; good enough for
/// Latin scripts. Non-Latin tenants will get visually OK results since
/// CJK / RTL renders character-by-character in SVG anyway.
fn wrap_description(text: &str, max_chars: usize, max_lines: usize) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut overflow = false;
    for word in text.split_whitespace() {
        if lines.len() >= max_lines {
            overflow = true;
            break;
        }
        if current.is_empty() {
            current.push_str(word);
            continue;
        }
        if current.chars().count() + 1 + word.chars().count() <= max_chars {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            if lines.len() >= max_lines {
                overflow = true;
                break;
            }
            current.push_str(word);
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    if overflow {
        if let Some(last) = lines.last_mut() {
            if last.chars().count() >= max_chars.saturating_sub(1) {
                while last.chars().count() > max_chars.saturating_sub(1) {
                    last.pop();
                }
            }
            last.push('…');
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Conservative XML text escape. We don't currently emit attribute-only
/// content through here, so `&apos;` / `&quot;` are over-cautious but
/// harmless.
fn escape_xml(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '&' => "&amp;".chars().collect::<Vec<_>>(),
            '<' => "&lt;".chars().collect(),
            '>' => "&gt;".chars().collect(),
            '"' => "&quot;".chars().collect(),
            '\'' => "&apos;".chars().collect(),
            // Strip control chars (XML 1.0 disallows them outright).
            c if (c as u32) < 0x20 && c != '\t' && c != '\n' && c != '\r' => vec![],
            c => vec![c],
        })
        .collect()
}

/// Attribute-context escape. Same as `escape_xml` for our purposes —
/// kept as a separate function to keep call sites honest about where
/// the bytes end up.
fn escape_attr(s: &str) -> String {
    escape_xml(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_within_budget() {
        let lines = wrap_description("a b c d e f g", 5, 4);
        assert!(lines.iter().all(|l| l.chars().count() <= 5));
    }

    #[test]
    fn ellipses_long_text() {
        let lines = wrap_description(
            "one two three four five six seven eight nine ten eleven twelve thirteen fourteen \
             fifteen sixteen",
            10,
            2,
        );
        assert_eq!(lines.len(), 2);
        assert!(lines[1].ends_with('…'), "got {lines:?}");
    }

    #[test]
    fn empty_description_yields_single_empty_line() {
        let lines = wrap_description("", 70, 4);
        assert_eq!(lines, vec![String::new()]);
    }

    #[test]
    fn escapes_xml_special_chars() {
        assert_eq!(escape_xml("a & <b>"), "a &amp; &lt;b&gt;");
    }

    #[test]
    fn render_svg_has_well_formed_root() {
        let svg = render_svg(SvgInputs {
            brand_name: "Acme",
            primary: "#2563eb",
            primary_fg: "#ffffff",
            bg: "#ffffff",
            fg: "#0f172a",
            muted_fg: "#475569",
            border: "#e2e8f0",
            skill_slug: "axum-handler",
            skill_version: "1.0.0",
            skill_description: "A Rust pattern for clean axum route handlers.",
            kind: "skill",
            logo_data_uri: None,
        });
        assert!(svg.starts_with("<?xml"), "missing xml decl: {svg:.60}");
        assert!(svg.contains("<svg"));
        assert!(svg.contains("axum-handler"));
        assert!(svg.contains("v1.0.0"));
        assert!(svg.contains("kind: skill"));
    }

    #[test]
    fn rejects_unknown_kind() {
        let err = resolve_kind(Some("hammer")).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }
}
