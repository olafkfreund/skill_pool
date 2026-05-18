use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::{Config, TenancyMode};
use crate::embedding::{self, SharedEmbedder};
use crate::storage::Storage;

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
    pub storage: Storage,
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

    pub fn embedder(&self) -> &SharedEmbedder {
        &self.inner.embedder
    }

    #[allow(dead_code)]
    pub fn origin_pattern(&self) -> &str {
        &self.inner.origin_pattern
    }
}
