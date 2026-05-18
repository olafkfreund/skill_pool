//! HTTP client for the registry. Phase 1 scaffold — endpoints filled in alongside server work.

use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};

use crate::config::RegistryConfig;

#[allow(dead_code)] // fields consumed once CLI commands wire to the server (#3)
pub struct Client {
    http: reqwest::Client,
    base: url::Url,
    tenant: String,
}

impl Client {
    pub fn new(reg: &RegistryConfig) -> Result<Self> {
        let mut headers = HeaderMap::new();
        if let Some(t) = &reg.token {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&format!("Bearer {t}"))?,
            );
        }
        // In shared-mode the server resolves tenant from subdomain; the header is the
        // dev-mode fallback and is harmless on production routers.
        headers.insert("x-skill-pool-tenant", HeaderValue::from_str(&reg.tenant)?);

        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("skill-pool/", env!("CARGO_PKG_VERSION")))
            .build()?;

        let base = url::Url::parse(&reg.url)?;
        Ok(Self {
            http,
            base,
            tenant: reg.tenant.clone(),
        })
    }

    #[allow(dead_code)] // used by doctor/search once those wire to the server (#3)
    pub fn tenant(&self) -> &str {
        &self.tenant
    }

    pub async fn healthz(&self) -> Result<serde_json::Value> {
        let url = self.base.join("/v1/healthz")?;
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("healthz returned {}", resp.status()));
        }
        Ok(resp.json().await?)
    }
}
