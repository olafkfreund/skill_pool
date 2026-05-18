//! HTTP client for the registry.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION};
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};

use crate::config::RegistryConfig;

#[allow(dead_code)] // fields used as commands wire to the server
pub struct Client {
    http: reqwest::Client,
    base: url::Url,
    tenant: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub slug: String,
    pub version: String,
    pub description: String,
    #[serde(default)]
    pub when_to_use: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct PublishMetadata<'a> {
    pub slug: &'a str,
    pub version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<&'a str>,
    #[serde(default)]
    pub tags: &'a [String],
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

    #[allow(dead_code)]
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

    #[allow(dead_code)] // wired into `search` command in the next CLI iteration
    pub async fn list_skills(&self, query: Option<&str>) -> Result<Vec<Skill>> {
        let mut url = self.base.join("/v1/skills")?;
        if let Some(q) = query {
            url.query_pairs_mut().append_pair("query", q);
        }
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("list_skills: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    pub async fn get_skill(&self, slug: &str) -> Result<Skill> {
        let url = self.base.join(&format!("/v1/skills/{slug}"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("skill `{slug}` not found"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("get_skill: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    pub async fn download_bundle(&self, slug: &str) -> Result<Bytes> {
        let url = self
            .base
            .join(&format!("/v1/skills/{slug}/bundle.tar.gz"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("download_bundle: {status} — {body}"));
        }
        Ok(resp.bytes().await?)
    }

    pub async fn publish(&self, metadata: PublishMetadata<'_>, bundle: Bytes) -> Result<Skill> {
        let url = self.base.join("/v1/skills")?;
        let metadata_json =
            serde_json::to_string(&metadata).context("serialise publish metadata")?;

        let form = Form::new().text("metadata", metadata_json).part(
            "bundle",
            Part::bytes(bundle.to_vec())
                .file_name(format!("{}.tar.gz", metadata.slug))
                .mime_str("application/gzip")?,
        );

        let resp = self.http.post(url).multipart(form).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("publish: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }
}
