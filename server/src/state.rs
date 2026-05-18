use std::sync::Arc;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::config::{Config, TenancyMode};

#[derive(Clone)]
pub struct AppState {
    inner: Arc<Inner>,
}

struct Inner {
    pub db: PgPool,
    pub tenancy: TenancyMode,
    pub origin_pattern: String,
    // storage: Arc<dyn opendal::Accessor> — wired in when bundle endpoints are implemented
}

impl AppState {
    pub async fn new(cfg: &Config) -> Result<Self> {
        let db = PgPoolOptions::new()
            .max_connections(20)
            .connect(&cfg.database_url)
            .await?;

        // Migrations run via `sqlx migrate run` ahead of boot in dev/prod.
        // Optionally enable here behind a feature flag once we have CI seed data:
        // sqlx::migrate!("./migrations").run(&db).await?;

        Ok(Self {
            inner: Arc::new(Inner {
                db,
                tenancy: cfg.resolved_tenancy(),
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

    #[allow(dead_code)]
    pub fn origin_pattern(&self) -> &str {
        &self.inner.origin_pattern
    }
}
