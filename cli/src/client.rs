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

#[derive(Debug, Clone, Deserialize)]
pub struct BootstrapResponse {
    #[allow(dead_code)] // server-echoed, useful for debug logs
    pub stack: Vec<String>,
    pub skills: Vec<String>,
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
    /// Cosine similarity to the semantic query when `?semantic=` was used.
    /// Absent for plain list / keyword responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub similarity: Option<f32>,
}

#[derive(Debug, Serialize)]
pub struct PublishMetadata<'a> {
    pub slug: &'a str,
    pub version: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<&'a str>,
    #[serde(default)]
    pub tags: &'a [String],
    /// Catalog kind: `skill` (default), `agent`, or `command`. Omitted
    /// from the wire payload when `None` so the server keeps its
    /// pre-Phase-5 default-skill behaviour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub struct CaptureMetadata<'a> {
    pub slug: &'a str,
    pub origin: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<&'a str>,
    #[serde(default)]
    pub tags: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub when_to_use: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapturedDraft {
    pub id: String,
    pub slug: String,
    pub status: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DecayCandidate {
    pub slug: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub use_count: i32,
    #[serde(default)]
    pub last_used_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DepEntry {
    pub slug: String,
    #[serde(default)]
    pub version_range: String,
    #[serde(default)]
    pub depth: i32,
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

    #[allow(dead_code)] // reserved for `skill-pool doctor` registry-reachability check
    pub async fn healthz(&self) -> Result<serde_json::Value> {
        let url = self.base.join("/v1/healthz")?;
        let resp = self.http.get(url).send().await?;
        if !resp.status().is_success() {
            return Err(anyhow!("healthz returned {}", resp.status()));
        }
        Ok(resp.json().await?)
    }

    pub async fn list_skills(
        &self,
        query: Option<&str>,
        tags: &[String],
        limit: Option<u32>,
        semantic: Option<&str>,
        min_similarity: Option<f32>,
    ) -> Result<Vec<Skill>> {
        let mut url = self.base.join("/v1/skills")?;
        {
            let mut q = url.query_pairs_mut();
            if let Some(s) = query {
                q.append_pair("query", s);
            }
            if !tags.is_empty() {
                q.append_pair("tags", &tags.join(","));
            }
            if let Some(n) = limit {
                q.append_pair("limit", &n.to_string());
            }
            if let Some(s) = semantic {
                q.append_pair("semantic", s);
            }
            if let Some(t) = min_similarity {
                q.append_pair("min_similarity", &t.to_string());
            }
        }
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("list_skills: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    pub async fn get_deps(&self, slug: &str) -> Result<Vec<DepEntry>> {
        let url = self.base.join(&format!("/v1/skills/{slug}/deps"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            // No published parent skill → no closure to walk. Treat the
            // same as "no deps" so the caller can skip without branching.
            return Ok(vec![]);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("get_deps: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    /// Fetch decay candidates (admin-scoped). Returns an empty vec on 401/403
    /// so `doctor` can soft-skip when the configured token lacks
    /// `tenant:admin` scope.
    pub async fn decay_candidates(&self) -> Result<Vec<DecayCandidate>> {
        let url = self.base.join("/v1/tenant/skills/decay")?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::UNAUTHORIZED
            || status == reqwest::StatusCode::FORBIDDEN
        {
            return Ok(vec![]);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("decay_candidates: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    /// Kind-aware metadata lookup. Mirrors the server's `?kind=` query
    /// param on `GET /v1/skills/{slug}`. `kind="skill"` is identical to
    /// the historical `get_skill` payload because the server defaults
    /// the query param to `"skill"` when omitted; we still forward it
    /// explicitly so the wire shape is the same for all three kinds.
    pub async fn get_skill_with_kind(&self, slug: &str, kind: &str) -> Result<Skill> {
        let mut url = self.base.join(&format!("/v1/skills/{slug}"))?;
        url.query_pairs_mut().append_pair("kind", kind);
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("{kind} `{slug}` not found"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("get_skill: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    pub async fn bootstrap(&self, stack: &[String]) -> Result<BootstrapResponse> {
        if stack.is_empty() {
            return Ok(BootstrapResponse {
                stack: vec![],
                skills: vec![],
            });
        }
        let mut url = self.base.join("/v1/bootstrap")?;
        url.query_pairs_mut().append_pair("stack", &stack.join(","));
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("bootstrap: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    /// Kind-aware bundle download. Same wire path with an additional
    /// `?kind=` query param so agents/commands round-trip too. The
    /// server defaults to `skill` when the param is absent, so omitting
    /// the kind on the server side still maps to `kind="skill"`.
    pub async fn download_bundle_with_kind(&self, slug: &str, kind: &str) -> Result<Bytes> {
        let mut url = self
            .base
            .join(&format!("/v1/skills/{slug}/bundle.tar.gz"))?;
        url.query_pairs_mut().append_pair("kind", kind);
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("download_bundle: {status} — {body}"));
        }
        Ok(resp.bytes().await?)
    }

    pub async fn submit_draft(
        &self,
        metadata: CaptureMetadata<'_>,
        bundle: Bytes,
    ) -> Result<CapturedDraft> {
        let url = self.base.join("/v1/drafts")?;
        let metadata_json =
            serde_json::to_string(&metadata).context("serialise draft metadata")?;

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
            return Err(anyhow!("submit_draft: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    /// POST a CLI-driven usage event to `/v1/usage`. Best-effort —
    /// the caller logs but does not propagate failures. `event` is
    /// `view` or `download`; v1 only ever sends `view` (from
    /// `skill-pool ensure`'s install path).
    ///
    /// `project_hash` is the SHA-256 (truncated) of the project root
    /// — anonymises which project / machine sent the event so we can
    /// dedup repeated events from the same install without storing a
    /// reversible identifier server-side.
    pub async fn send_usage_event(
        &self,
        skill_id: &str,
        kind: &str,
        event: &str,
        project_hash: &str,
    ) -> Result<()> {
        let url = self.base.join("/v1/usage")?;
        let body = serde_json::json!({
            "skill_id": skill_id,
            "kind": kind,
            "event": event,
            "project_hash": project_hash,
        });
        let resp = self.http.post(url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("send_usage_event: {status} — {body}"));
        }
        Ok(())
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
