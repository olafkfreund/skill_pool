use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

/// How the server resolves tenant identity from requests.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[allow(dead_code)] // Dedicated.tenant_slug consumed by tenant.rs's match arm (#3)
pub enum TenancyMode {
    /// Multi-tenant: tenant resolved from subdomain or X-Skill-Pool-Tenant header.
    Shared,
    /// Single-tenant deploy: tenant_id pinned at startup; no subdomain routing required.
    Dedicated { tenant_slug: String },
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // fields consumed as routes/storage land (#3)
pub struct Config {
    #[serde(default = "default_bind")]
    pub bind: String,

    #[serde(default)]
    pub tenancy_mode: TenancyModeRaw,

    pub database_url: String,

    /// Optional read-replica DSN. When set, read-only handlers route to
    /// this pool instead of `database_url`. Replication lag means a
    /// publish-then-list within milliseconds can see stale state — for
    /// the catalog that's acceptable (Postgres async replication is
    /// typically <100ms).
    pub database_read_url: Option<String>,

    /// sqlx connection-pool max size for the primary DB. The read pool
    /// (if configured) uses the same cap.
    #[serde(default = "default_db_pool_size")]
    pub db_pool_size: u32,

    #[serde(default = "default_storage_uri")]
    pub storage_uri: String,

    /// Public origin pattern used when constructing absolute URLs.
    /// `{tenant}` is substituted with the tenant slug in shared mode.
    #[serde(default = "default_origin_pattern")]
    pub origin_pattern: String,

    /// Phase 5 — embedding-based dedup. Off by default so a default build
    /// (without `--features fastembed`) is fully functional without
    /// pgvector or HuggingFace network.
    #[serde(default)]
    pub embedding: EmbeddingConfig,

    /// Optional Redis connection URL used by the server-side
    /// read-through caches (theme / per-request auth) and the rate
    /// limiter. Accepts both `redis://` and `rediss://` (TLS). Unset →
    /// caches are no-ops and every request goes straight to Postgres.
    /// Connection failure at startup is logged + treated as "no Redis"
    /// rather than a hard error — see `state::AppState::new`.
    #[serde(default)]
    pub redis_url: Option<String>,

    /// Master switch for the Redis-backed job queue (#10 §D). When
    /// unset we default to `Some(true)` if `redis_url` is configured,
    /// because the only reason to disable it on a Redis-enabled
    /// deployment is the bring-up window where an operator wants the
    /// old inline behaviour. Setting this to `false` forces the
    /// inline path on every consumer even when Redis is available.
    #[serde(default)]
    pub queue_enabled: Option<bool>,

    /// Interval (in seconds) between background decay sweeps that flip
    /// long-stale skills to `status = 'archive_candidate'` (#7 lifecycle).
    /// Defaults to 24h. Set to `0` to disable the sweep entirely — the
    /// on-demand `/v1/tenant/skills/decay` endpoint continues to work.
    #[serde(default = "default_decay_interval")]
    pub decay_check_interval_secs: u32,

    /// Optional path to a Git repo to mirror catalog publishes into.
    /// Best-effort: when unset, the publish handlers skip the Git side
    /// entirely. When set but the path is missing or `git` is not
    /// installed, the publish still succeeds — the failure is logged
    /// and swallowed so Postgres (source of truth) stays authoritative.
    ///
    /// Env: `SKILL_POOL_GIT_REPO_PATH`. See `docs/lifecycle.md` for the
    /// on-disk layout and operator notes.
    #[serde(default)]
    pub git_repo_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EmbeddingConfig {
    /// Master switch. When false, the server uses `NullEmbedder` — schema
    /// columns stay NULL, dedup is a no-op.
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TenancyModeRaw {
    /// "shared" or "dedicated"
    #[serde(default = "default_mode")]
    pub mode: String,
    pub tenant_slug: Option<String>,
}

fn default_bind() -> String {
    "0.0.0.0:8080".into()
}
fn default_db_pool_size() -> u32 {
    20
}
fn default_storage_uri() -> String {
    "fs:///var/lib/skill-pool/storage".into()
}
fn default_origin_pattern() -> String {
    "https://{tenant}.skill-pool.example.com".into()
}
fn default_mode() -> String {
    "shared".into()
}
fn default_decay_interval() -> u32 {
    86_400
}

impl Config {
    pub fn load() -> Result<Self> {
        use figment::{providers::Env, Figment};

        let cfg: Config = Figment::new()
            .merge(Env::prefixed("SKILL_POOL_").split("__"))
            .extract()?;

        Ok(cfg)
    }

    pub fn resolved_tenancy(&self) -> TenancyMode {
        match self.tenancy_mode.mode.as_str() {
            "dedicated" => TenancyMode::Dedicated {
                tenant_slug: self
                    .tenancy_mode
                    .tenant_slug
                    .clone()
                    .unwrap_or_else(|| "default".into()),
            },
            _ => TenancyMode::Shared,
        }
    }
}
