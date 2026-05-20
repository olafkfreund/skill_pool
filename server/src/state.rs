use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::{Mutex, RwLock};
use uuid::Uuid;

use crate::cache;
use crate::config::{Config, TenancyMode};
use crate::email_branding::TransportCache as EmailTransportCache;
use crate::embedding::{self, SharedEmbedder};
use crate::queue;
use crate::storage::Storage;
use crate::tenant::TenantCtx;

/// How often the background task refreshes the host→tenant cache from
/// the DB. 60s is a deliberate tradeoff: short enough that an operator
/// activating a verified domain sees traffic flow within a minute, long
/// enough that the cache load query (a single indexed SELECT) is
/// effectively free per process.
pub(crate) const CUSTOM_DOMAIN_REFRESH_SECS: u64 = 60;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    pub db: PgPool,
    /// Optional read-replica pool. Read-only handlers prefer this when
    /// set; everything else uses `db`.
    pub db_read: Option<PgPool>,
    pub tenancy: TenancyMode,
    /// Default bundle storage from `SKILL_POOL_STORAGE_URI`. Used by
    /// tenants that have not set their own `tenants.storage_uri` override.
    pub storage: Storage,
    /// Lazy cache of per-tenant storage backends. Populated on first use
    /// when a tenant row has `storage_uri IS NOT NULL`. Entries never
    /// evict: an unbounded `HashMap` is fine because the cap is the
    /// number of distinct tenants with overrides, which is tiny in
    /// practice (an `opendal::Operator` is ~1 KB resident).
    pub tenant_storage: Mutex<HashMap<Uuid, Arc<Storage>>>,
    /// Lazy cache of per-tenant branded SMTP transports
    /// (`tenant_email_branding`). Built on first send. Same eviction
    /// story as `tenant_storage`: never automatic; admin endpoints
    /// invalidate after a PUT/DELETE.
    pub email_transport: Arc<EmailTransportCache>,
    /// Hostname → tenant_id cache for `tenant_custom_domains` rows whose
    /// status is `verified` or `active`. Populated at startup and
    /// refreshed every `CUSTOM_DOMAIN_REFRESH_SECS` seconds by a
    /// background task; mutating endpoints also call `bump_custom_domains`
    /// to force an immediate reload so admins see their changes flow
    /// without waiting out the TTL.
    ///
    /// `RwLock` because the read path is per-request (every API call
    /// after a cache hit) and the write path is the periodic refresher.
    pub custom_domains: RwLock<HashMap<String, Uuid>>,
    pub embedder: SharedEmbedder,
    #[allow(dead_code)]
    pub origin_pattern: String,
    /// Optional Redis client for read-through caches (theme, auth) and
    /// the rate limiter (#9 §L36, #10 §A, #8 §L20). `None` when
    /// `SKILL_POOL_REDIS_URL` is unset or the initial connect failed —
    /// callers fall back to the direct-DB path / fail-open behaviour.
    /// `cache::Redis` is already `Arc<ConnectionManager>`, so cloning
    /// the inner value across `.await` points is cheap.
    pub redis: Option<cache::Redis>,
    /// Redis-backed job queue (#10 §D). Built when Redis is available
    /// AND `cfg.queue_enabled` is unset or true. Consumers (currently
    /// the email-notification path in `notify.rs`) branch on
    /// `state.queue()` and fall back to inline behaviour when this is
    /// `None`, preserving the pre-queue contract for Redis-off deploys.
    pub queue: Option<Arc<queue::Queue>>,
    /// Optional path to a Git repo mirroring every catalog publish
    /// (#6 Phase 4 two-way sync). `Some(path)` when
    /// `SKILL_POOL_GIT_REPO_PATH` is set in config; publish handlers
    /// kick off a detached `git_sync::commit_skill` task. Best-effort:
    /// a missing repo or unavailable `git` logs a warning and lets
    /// the publish succeed regardless.
    pub git_repo_path: Option<PathBuf>,
    /// Shared HTTP client used for project-plan URL fetches and outbound
    /// webhook delivery. Configured with a 10s timeout and 5-redirect cap.
    /// Cloning is cheap (`reqwest::Client` is `Arc<Inner>`).
    pub http_client: reqwest::Client,
}

async fn connect_pool(url: &str, max: u32) -> Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(max)
        .connect(url)
        .await?)
}

/// Build the shared HTTP client for outbound requests (plan URL fetches,
/// webhook delivery). 10s timeout, max 5 redirects, rustls TLS — no OpenSSL.
fn build_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .expect("build shared HTTP client")
}

/// Build the Redis client when `redis_url` is set. Connection failure
/// degrades to "no cache" rather than killing startup — Redis is a
/// performance booster, not a hard dependency. The rate-limiter built
/// on top by the sister subagent uses the same `Option<&Redis>` surface
/// and shares the same fallback.
async fn maybe_redis(cfg: &Config) -> Option<cache::Redis> {
    let url = cfg.redis_url.as_deref().filter(|s| !s.is_empty())?;
    match cache::connect(url).await {
        Ok(r) => {
            tracing::info!("redis cache connected");
            Some(r)
        }
        Err(e) => {
            tracing::warn!(error = %e, "redis connect failed; running without cache");
            None
        }
    }
}

/// Build the shared job queue. Requires a live Redis client AND
/// `queue_enabled` either unset (default-on) or explicitly true.
/// Returns `None` so callers naturally fall back to inline delivery.
fn maybe_queue(redis: Option<&cache::Redis>, cfg: &Config) -> Option<Arc<queue::Queue>> {
    let redis = redis?;
    if matches!(cfg.queue_enabled, Some(false)) {
        tracing::info!("queue_enabled=false; job queue disabled");
        return None;
    }
    let q = queue::Queue::new(redis.clone(), "default");
    tracing::info!(queue = "default", "job queue initialised");
    Some(Arc::new(q))
}

/// Build the read pool when `database_read_url` is set. Failure here
/// degrades to "no read pool" rather than killing startup — the replica
/// being temporarily unreachable shouldn't take the primary deployment
/// down.
async fn maybe_read_pool(cfg: &Config) -> Option<PgPool> {
    let url = cfg.database_read_url.as_ref()?;
    match connect_pool(url, cfg.db_pool_size).await {
        Ok(pool) => {
            tracing::info!(pool_size = cfg.db_pool_size, "read-replica pool connected");
            Some(pool)
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                "read-replica pool connection failed; falling back to primary for reads"
            );
            None
        }
    }
}

impl AppState {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let db = connect_pool(&cfg.database_url, cfg.db_pool_size).await?;
        let db_read = maybe_read_pool(cfg).await;
        let redis = maybe_redis(cfg).await;
        let queue = maybe_queue(redis.as_ref(), cfg);

        let storage = Storage::from_uri(&cfg.storage_uri)?;
        let embedder = embedding::from_config(&cfg.embedding)?;

        let state = Self {
            inner: Arc::new(Inner {
                db,
                db_read,
                tenancy: cfg.resolved_tenancy(),
                storage,
                tenant_storage: Mutex::new(HashMap::new()),
                email_transport: Arc::new(EmailTransportCache::new()),
                custom_domains: RwLock::new(HashMap::new()),
                embedder,
                origin_pattern: cfg.origin_pattern.clone(),
                redis,
                queue,
                git_repo_path: cfg.git_repo_path.clone(),
                http_client: build_http_client(),
            }),
        };
        // Warm the cache once synchronously so the first request after
        // startup doesn't 401 on a custom-domain host. Failure here is
        // not fatal: the cache stays empty and host-based requests fall
        // back to subdomain resolution.
        if let Err(e) = state.refresh_custom_domains().await {
            tracing::warn!(error = ?e, "initial custom-domain cache load failed; will retry");
        }
        Ok(state)
    }

    /// Build an `AppState` with an explicit embedder. Used by tests that
    /// want to inject a deterministic stub without going through env
    /// configuration.
    pub async fn new_with_embedder(cfg: &Config, embedder: SharedEmbedder) -> Result<Self> {
        let db = connect_pool(&cfg.database_url, cfg.db_pool_size).await?;
        let db_read = maybe_read_pool(cfg).await;
        let redis = maybe_redis(cfg).await;
        let queue = maybe_queue(redis.as_ref(), cfg);
        let storage = Storage::from_uri(&cfg.storage_uri)?;
        let state = Self {
            inner: Arc::new(Inner {
                db,
                db_read,
                tenancy: cfg.resolved_tenancy(),
                storage,
                tenant_storage: Mutex::new(HashMap::new()),
                email_transport: Arc::new(EmailTransportCache::new()),
                custom_domains: RwLock::new(HashMap::new()),
                embedder,
                origin_pattern: cfg.origin_pattern.clone(),
                redis,
                queue,
                git_repo_path: cfg.git_repo_path.clone(),
                http_client: build_http_client(),
            }),
        };
        if let Err(e) = state.refresh_custom_domains().await {
            tracing::warn!(error = ?e, "initial custom-domain cache load failed; will retry");
        }
        Ok(state)
    }

    pub fn db(&self) -> &PgPool {
        &self.inner.db
    }

    /// Pool to use for read-only queries. Returns the read replica when
    /// configured, otherwise the primary. Use this for `SELECT`-only
    /// handlers; never for queries that write, even best-effort.
    pub fn db_read(&self) -> &PgPool {
        self.inner.db_read.as_ref().unwrap_or(&self.inner.db)
    }

    pub fn tenancy(&self) -> &TenancyMode {
        &self.inner.tenancy
    }

    pub fn storage(&self) -> &Storage {
        &self.inner.storage
    }

    /// Return the bundle-storage backend to use for a tenant context.
    ///
    /// When `tenants.storage_uri IS NULL` (the default — every existing
    /// tenant before migration `0018`) this returns an `Arc` wrapping the
    /// process-wide default backend so callers have a uniform type.
    ///
    /// When `storage_uri` is set, the backend is built on first use and
    /// cached on `AppState`. Cache eviction is a non-goal (tenant count
    /// in the low hundreds; each entry is ~1 KB).
    ///
    /// Failure modes:
    ///   * The tenant row's `storage_uri` is malformed — `Storage::from_uri`
    ///     returns Err. The CHECK constraint added by migration `0018`
    ///     catches obvious typos at write time; admin CLI also validates.
    ///   * The backend itself is unreachable — same failure surface as the
    ///     default backend (`opendal` reports on first operation).
    pub async fn storage_for(&self, tenant: &TenantCtx) -> Result<Arc<Storage>> {
        // Hot path: cached.
        if let Some(cached) = {
            let cache = self.inner.tenant_storage.lock().await;
            cache.get(&tenant.tenant_id).cloned()
        } {
            return Ok(cached);
        }

        // Cold path: look up override from DB, build, insert.
        let row = sqlx::query!(
            "SELECT storage_uri FROM tenants WHERE id = $1",
            tenant.tenant_id,
        )
        .fetch_optional(&self.inner.db)
        .await?;
        let override_uri = row.and_then(|r| r.storage_uri);

        let entry: Arc<Storage> = match override_uri {
            None => {
                // Wrap default in Arc so all paths return the same type.
                // Note: we clone the `Storage` struct (cheap — it's just
                // an `opendal::Operator` handle).
                Arc::new(self.inner.storage.clone())
            }
            Some(uri) => {
                let s = Storage::from_uri(&uri)
                    .map_err(|e| anyhow::anyhow!("tenant `{}` storage_uri: {e}", tenant.tenant_slug))?;
                Arc::new(s)
            }
        };

        let mut cache = self.inner.tenant_storage.lock().await;
        // Double-check: another caller may have populated between our two
        // critical sections. Use their entry to avoid two Operators for
        // the same tenant.
        if let Some(existing) = cache.get(&tenant.tenant_id) {
            return Ok(existing.clone());
        }
        cache.insert(tenant.tenant_id, entry.clone());
        Ok(entry)
    }

    /// Drop a tenant's cached storage entry. Call after `admin tenant-region-set`
    /// or any other mutation that changes `tenants.storage_uri`.
    #[allow(dead_code)] // wired in once a runtime admin endpoint exists
    pub async fn invalidate_tenant_storage(&self, tenant_id: Uuid) {
        self.inner.tenant_storage.lock().await.remove(&tenant_id);
    }

    pub fn embedder(&self) -> &SharedEmbedder {
        &self.inner.embedder
    }

    /// Shared Redis client for read-through caches + the rate limiter.
    ///
    /// Returns `Some(&Arc<ConnectionManager>)` when `SKILL_POOL_REDIS_URL`
    /// is set and the initial connect succeeded; `None` otherwise.
    /// Callers `.clone()` the inner `Arc` cheaply when they need to move
    /// the handle across an `.await`.
    pub fn redis(&self) -> Option<&cache::Redis> {
        self.inner.redis.as_ref()
    }

    /// Shared Redis-backed job queue (#10 §D). `Some` when Redis is
    /// healthy and `queue_enabled` isn't explicitly false; `None`
    /// otherwise. Callers branch on this and use the inline
    /// fallback path when it's `None`, identical to the pre-queue
    /// behaviour. Cloning the `Arc` to move it across an `.await` is
    /// cheap.
    pub fn queue(&self) -> Option<&Arc<queue::Queue>> {
        self.inner.queue.as_ref()
    }

    /// Path of the optional Git mirror repo. `Some` when
    /// `SKILL_POOL_GIT_REPO_PATH` is set; callers spawn a detached
    /// `git_sync::commit_skill` after a successful publish so the
    /// catalog has a human-readable history on disk. Failure is logged
    /// and swallowed — Postgres remains the source of truth.
    pub fn git_repo_path(&self) -> Option<&Path> {
        self.inner.git_repo_path.as_deref()
    }

    /// Shared HTTP client for outbound requests (project-plan URL fetches,
    /// webhook delivery). `reqwest::Client` is internally `Arc<Inner>` so
    /// cloning across `.await` points is cheap.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.inner.http_client
    }

    /// Test-only: build an `AppState` with an explicit Redis handle.
    /// Used by `tests/rate_limits.rs` to point the limiter at a
    /// testcontainer instead of relying on `SKILL_POOL_REDIS_URL`.
    #[allow(dead_code)]
    pub async fn new_with_redis(cfg: &Config, redis: cache::Redis) -> Result<Self> {
        let db = connect_pool(&cfg.database_url, cfg.db_pool_size).await?;
        let db_read = maybe_read_pool(cfg).await;
        let storage = Storage::from_uri(&cfg.storage_uri)?;
        let embedder = embedding::from_config(&cfg.embedding)?;
        let some_redis = Some(redis);
        let queue = maybe_queue(some_redis.as_ref(), cfg);
        let state = Self {
            inner: Arc::new(Inner {
                db,
                db_read,
                tenancy: cfg.resolved_tenancy(),
                storage,
                tenant_storage: Mutex::new(HashMap::new()),
                email_transport: Arc::new(EmailTransportCache::new()),
                custom_domains: RwLock::new(HashMap::new()),
                embedder,
                origin_pattern: cfg.origin_pattern.clone(),
                redis: some_redis,
                queue,
                git_repo_path: cfg.git_repo_path.clone(),
                http_client: build_http_client(),
            }),
        };
        if let Err(e) = state.refresh_custom_domains().await {
            tracing::warn!(error = ?e, "initial custom-domain cache load failed; will retry");
        }
        Ok(state)
    }


    /// Shared cache of per-tenant branded SMTP transports.
    pub fn email_transport(&self) -> &Arc<EmailTransportCache> {
        &self.inner.email_transport
    }

    #[allow(dead_code)]
    pub fn origin_pattern(&self) -> &str {
        &self.inner.origin_pattern
    }

    // ------------------------------------------------------------------
    // Custom-domain cache
    // ------------------------------------------------------------------

    /// Resolve a request `Host` value (lower-cased, port stripped) to a
    /// tenant_id via the in-process cache. Returns `None` when the host
    /// is not a known custom domain — callers fall back to the
    /// subdomain/header logic in `slug_from_request`.
    pub async fn custom_domain_tenant(&self, host: &str) -> Option<Uuid> {
        let cache = self.inner.custom_domains.read().await;
        cache.get(host).copied()
    }

    /// Replace the cache contents with the current set of verified/active
    /// rows. Called at startup, on every mutating custom-domain endpoint
    /// (so admins see flips immediately), and on a background interval.
    pub async fn refresh_custom_domains(&self) -> Result<()> {
        // NOTE: no tenant_id filter — this is a global cache load that intentionally
        // reads all tenants' custom domain rows to build the hostname→tenant_id map.
        let rows = sqlx::query!(
            "SELECT hostname, tenant_id \
             FROM tenant_custom_domains \
             WHERE status IN ('verified', 'active')",
        )
        .fetch_all(&self.inner.db)
        .await?;
        let mut next = HashMap::with_capacity(rows.len());
        for r in rows {
            next.insert(r.hostname.to_lowercase(), r.tenant_id);
        }
        let mut cache = self.inner.custom_domains.write().await;
        *cache = next;
        Ok(())
    }

    /// Spawn a detached task that calls `refresh_custom_domains` on a
    /// fixed interval. Returned by `new` callers that want background
    /// refresh; tests skip it (a one-shot warm is plenty there).
    pub fn spawn_custom_domain_refresher(&self) -> tokio::task::JoinHandle<()> {
        let state = self.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(CUSTOM_DOMAIN_REFRESH_SECS));
            // The first tick fires immediately, but we already warmed in
            // `new`; skip it to avoid a double-load at startup.
            tick.tick().await;
            loop {
                tick.tick().await;
                if let Err(e) = state.refresh_custom_domains().await {
                    tracing::warn!(error = ?e, "custom-domain cache refresh failed");
                }
            }
        })
    }
}
