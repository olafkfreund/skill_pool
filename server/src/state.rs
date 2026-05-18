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
    pub tenancy: TenancyMode,
    pub storage: Storage,
    pub embedder: SharedEmbedder,
    #[allow(dead_code)]
    pub origin_pattern: String,
}

impl AppState {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(20)
            .connect(&cfg.database_url)
            .await?;

        let storage = Storage::from_uri(&cfg.storage_uri)?;
        let embedder = embedding::from_config(&cfg.embedding)?;

        Ok(Self {
            inner: Arc::new(Inner {
                db,
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
        let db = PgPoolOptions::new()
            .max_connections(20)
            .connect(&cfg.database_url)
            .await?;
        let storage = Storage::from_uri(&cfg.storage_uri)?;
        Ok(Self {
            inner: Arc::new(Inner {
                db,
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
