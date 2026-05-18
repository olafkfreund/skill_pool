use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::{Config, TenancyMode};
use crate::storage::Storage;

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    pub db: PgPool,
    pub tenancy: TenancyMode,
    pub storage: Storage,
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

        Ok(Self {
            inner: Arc::new(Inner {
                db,
                tenancy: cfg.resolved_tenancy(),
                storage,
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

    #[allow(dead_code)]
    pub fn origin_pattern(&self) -> &str {
        &self.inner.origin_pattern
    }
}
