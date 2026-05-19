use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

use crate::config::{Config, TenancyMode};
use crate::embedding::{self, SharedEmbedder};
use crate::storage::Storage;
use crate::tenant::TenantCtx;

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
    pub embedder: SharedEmbedder,
    #[allow(dead_code)]
    pub origin_pattern: String,
}

async fn connect_pool(url: &str, max: u32) -> Result<PgPool> {
    Ok(PgPoolOptions::new()
        .max_connections(max)
        .connect(url)
        .await?)
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

        let storage = Storage::from_uri(&cfg.storage_uri)?;
        let embedder = embedding::from_config(&cfg.embedding)?;

        Ok(Self {
            inner: Arc::new(Inner {
                db,
                db_read,
                tenancy: cfg.resolved_tenancy(),
                storage,
                tenant_storage: Mutex::new(HashMap::new()),
                embedder,
                origin_pattern: cfg.origin_pattern.clone(),
            }),
        })
    }

    /// Build an `AppState` with an explicit embedder. Used by tests that
    /// want to inject a deterministic stub without going through env
    /// configuration.
    pub async fn new_with_embedder(cfg: &Config, embedder: SharedEmbedder) -> Result<Self> {
        let db = connect_pool(&cfg.database_url, cfg.db_pool_size).await?;
        let db_read = maybe_read_pool(cfg).await;
        let storage = Storage::from_uri(&cfg.storage_uri)?;
        Ok(Self {
            inner: Arc::new(Inner {
                db,
                db_read,
                tenancy: cfg.resolved_tenancy(),
                storage,
                tenant_storage: Mutex::new(HashMap::new()),
                embedder,
                origin_pattern: cfg.origin_pattern.clone(),
            }),
        })
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
        let row: Option<(Option<String>,)> =
            sqlx::query_as("SELECT storage_uri FROM tenants WHERE id = $1")
                .bind(tenant.tenant_id)
                .fetch_optional(&self.inner.db)
                .await?;
        let override_uri = row.and_then(|(u,)| u);

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

    #[allow(dead_code)]
    pub fn origin_pattern(&self) -> &str {
        &self.inner.origin_pattern
    }
}
