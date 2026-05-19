//! Per-tenant CLI startup banner (#9 / Enterprise).
//!
//! Called from `main.rs` after `Cli::parse()` and before the subcommand
//! match. The function is fire-and-forget: any error (no registry, no
//! network, slow server, parse failure) is silently swallowed because a
//! cosmetic greeting must not block real work.
//!
//! ## Why we print to stderr
//! Pipelines slurp stdout (`skill-pool search foo | grep bar`). A banner
//! on stdout would corrupt their input. stderr is for humans by
//! convention; piping `2>/dev/null` suppresses it cleanly.
//!
//! ## When we DON'T print
//! 1. `stdout` is not a TTY — we're being scripted; banner would just
//!    be noise in CI logs and (worse) get captured if someone redirects
//!    both streams.
//! 2. `SKILL_POOL_NO_BANNER=1` is set — explicit operator opt-out for
//!    quiet automation environments.
//! 3. `~/.skill-pool/banner-shown` was modified less than 24h ago —
//!    once-per-shell-session is the wrong granularity for a CLI that
//!    gets re-invoked many times per minute; once per day per machine
//!    matches the "remind me we're on Acme's registry" intent without
//!    becoming spam.
//! 4. No registry configured — nothing to fetch.
//!
//! ## Caching philosophy
//! We don't cache the banner *body* (that would force us to invalidate
//! when an admin updates it). We only cache "I showed something
//! recently" via the mtime on a sentinel file. When 24h is up, the next
//! call re-fetches and re-displays whatever the server currently
//! returns. The 1.5s request timeout caps the worst-case startup
//! penalty when the registry is unreachable.

use std::io::IsTerminal;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::Deserialize;

use crate::config::Config;

const SENTINEL_REL: &str = ".skill-pool/banner-shown";
const DEDUP_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
const FETCH_TIMEOUT: Duration = Duration::from_millis(1500);
const OPT_OUT_ENV: &str = "SKILL_POOL_NO_BANNER";

#[derive(Debug, Deserialize)]
struct Banner {
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

/// Best-effort: print the tenant's branded banner to stderr on first
/// run-per-day in a TTY. Never returns an error; never panics on a
/// missing config dir, a slow server, or a broken response.
pub async fn show_if_due(cfg: &Config) {
    if !should_show() {
        return;
    }
    let Some(reg) = cfg.registry.as_ref() else {
        return;
    };
    let Some(banner) = fetch(reg).await else {
        return;
    };
    match (banner.text.as_deref(), banner.url.as_deref()) {
        (Some(t), Some(u)) if !t.is_empty() && !u.is_empty() => {
            eprintln!("{t}");
            eprintln!("{u}");
        }
        (Some(t), _) if !t.is_empty() => {
            eprintln!("{t}");
        }
        (_, Some(u)) if !u.is_empty() => {
            eprintln!("{u}");
        }
        // Both null/empty: server has no banner for this tenant. Touch
        // the sentinel anyway so we don't keep hitting the network for
        // a tenant that explicitly has nothing configured.
        _ => {}
    }
    touch_sentinel();
}

fn should_show() -> bool {
    if std::env::var(OPT_OUT_ENV).is_ok() {
        return false;
    }
    if !std::io::stdout().is_terminal() {
        return false;
    }
    if let Some(path) = sentinel_path() {
        if let Ok(meta) = std::fs::metadata(&path) {
            if let Ok(modified) = meta.modified() {
                if let Ok(elapsed) = SystemTime::now().duration_since(modified) {
                    if elapsed < DEDUP_WINDOW {
                        return false;
                    }
                }
            }
        }
    }
    true
}

async fn fetch(reg: &crate::config::RegistryConfig) -> Option<Banner> {
    let base = url::Url::parse(&reg.url).ok()?;
    let url = base.join("/v1/tenant/profile/banner").ok()?;
    let http = reqwest::Client::builder()
        .timeout(FETCH_TIMEOUT)
        .user_agent(concat!("skill-pool/", env!("CARGO_PKG_VERSION")))
        .build()
        .ok()?;
    let resp = http
        .get(url)
        .header("x-skill-pool-tenant", &reg.tenant)
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    resp.json::<Banner>().await.ok()
}

fn sentinel_path() -> Option<PathBuf> {
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    Some(home.join(SENTINEL_REL))
}

fn touch_sentinel() {
    let Some(path) = sentinel_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // Open with create+truncate to bump mtime to "now". Content is
    // irrelevant; we only ever read the mtime.
    let _ = std::fs::write(&path, b"");
}
