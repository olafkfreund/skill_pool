//! Theme endpoints.
//!
//! - GET /v1/theme — tenant-scoped, no auth needed (login page renders the
//!   tenant's brand before any user has authenticated).
//! - PUT /v1/theme — requires `tenant:admin` scope.

use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::audit;
use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tenant::TenantCtx;

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
}

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
            muted_fg: "#64748b".into(),
            border: "#e2e8f0".into(),
            radius: "0.5rem".into(),
            logo_uri: None,
            footer_branding: true,
        }
    }
}

pub async fn get_theme(State(state): State<AppState>, tenant: TenantCtx) -> AppResult<Json<Theme>> {
    let row: Option<ThemeRow> = sqlx::query_as(
        "SELECT brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, \
                border, radius, logo_uri, footer_branding \
         FROM tenant_theme WHERE tenant_id = $1",
    )
    .bind(tenant.tenant_id)
    .fetch_optional(state.db())
    .await?;

    let theme = row
        .map(Theme::from)
        .unwrap_or_else(|| Theme::default_for(&tenant.tenant_slug));
    Ok(Json(theme))
}

pub async fn put_theme(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(body): Json<Theme>,
) -> AppResult<(StatusCode, Json<Theme>)> {
    require_scope(&caller.scope, "tenant:admin")?;
    validate(&body)?;

    sqlx::query(
        "INSERT INTO tenant_theme \
           (tenant_id, brand_name, primary_, primary_fg, accent, bg, fg, muted, muted_fg, border, radius, logo_uri, footer_branding) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13) \
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
           footer_branding = EXCLUDED.footer_branding",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&body.brand_name)
    .bind(&body.primary)
    .bind(&body.primary_fg)
    .bind(&body.accent)
    .bind(&body.bg)
    .bind(&body.fg)
    .bind(&body.muted)
    .bind(&body.muted_fg)
    .bind(&body.border)
    .bind(&body.radius)
    .bind(body.logo_uri.as_deref())
    .bind(body.footer_branding)
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

#[derive(sqlx::FromRow)]
struct ThemeRow {
    brand_name: String,
    #[sqlx(rename = "primary_")]
    primary: String,
    primary_fg: String,
    accent: String,
    bg: String,
    fg: String,
    muted: String,
    muted_fg: String,
    border: String,
    radius: String,
    logo_uri: Option<String>,
    footer_branding: bool,
}

impl From<ThemeRow> for Theme {
    fn from(r: ThemeRow) -> Self {
        Self {
            brand_name: r.brand_name,
            primary: r.primary,
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
        }
    }
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
}
