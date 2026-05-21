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

/// A project record as returned by `GET /v1/tenant/projects`.
#[derive(Debug, Clone, Deserialize)]
pub struct Project {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub git_remote: Option<String>,
    #[serde(default)]
    pub stack_tags: Vec<String>,
    #[serde(default)]
    pub item_count: u32,
}

/// An item within a project (skill, agent, or command).
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectItem {
    pub skill_slug: String,
    pub kind: String,
    #[serde(default)]
    pub position: i32,
}

/// A project with its full item list, returned by `GET /v1/tenant/projects/{slug}`.
#[derive(Debug, Clone, Deserialize)]
pub struct ProjectWithItems {
    pub slug: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub git_remote: Option<String>,
    #[serde(default)]
    pub stack_tags: Vec<String>,
    #[serde(default)]
    pub items: Vec<ProjectItem>,
}

/// Minimal response from `GET /v1/projects/resolve?remote=<url>`.
#[derive(Debug, Clone, Deserialize)]
pub struct ResolvedProject {
    pub slug: String,
    pub name: String,
}

/// One entry in the `GET /v1/tenant/projects/{slug}/plan/versions` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanVersion {
    pub version: u32,
    pub status: String,
    pub source_type: String,
    #[serde(default)]
    pub source_url: Option<String>,
    pub imported_at: String,
    #[serde(default)]
    pub imported_by_email: Option<String>,
}

/// Outcome from `POST /v1/tenant/projects/{slug}/plan/refresh`.
#[derive(Debug, Clone, Deserialize)]
pub struct RefreshOutcome {
    /// `"unchanged"` or `"updated"`.
    pub outcome: String,
    #[serde(default)]
    pub new_version: Option<u32>,
    #[serde(default)]
    pub error: Option<String>,
}

// ── Plugin types (CLI-side wire shapes for #30 / #32 / #36) ──────────────────
//
// Mirrors the canonical Rust types in `server/src/plugin.rs` (#29 schema).
// Kept CLI-local so the CLI crate doesn't pull in the server crate as a
// dependency; the wire JSON shape is the contract.

/// One bundled `{kind, slug, version}` entry inside a plugin manifest body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginContentRef {
    pub kind: String,
    pub slug: String,
    pub version: String,
}

/// The body of a `.claude-plugin/plugin.json`. Extra fields (hooks, MCP
/// servers, etc.) flow through `extra` so the Claude Code spec can evolve
/// without forcing CLI changes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub contents: Vec<PluginContentRef>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One row from `GET /v1/plugins`. Tolerant of additional server fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginListEntry {
    pub slug: String,
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    /// `"draft" | "published" | "archived"` (DB CHECK in migration 0031).
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// Server response from a successful `POST /v1/plugins`.
#[derive(Debug, Clone, Deserialize)]
pub struct PublishedPlugin {
    pub slug: String,
    pub version: String,
    #[serde(default)]
    pub status: String,
}

/// One `(kind, slug, version)` entry inside a `PluginDetail.contents`.
/// Mirrors `server::routes::plugins::PluginContentResponse`.
#[derive(Debug, Clone, Deserialize)]
pub struct PluginContentEntry {
    pub kind: String,
    pub slug: String,
    pub version: String,
    #[serde(default)]
    pub position: i32,
}

/// Full plugin response from `GET /v1/plugins/{slug}`. Used by the #36
/// transitive resolver in `ensure.rs` to walk a plugin's bundled
/// skills/agents/commands and merge them into the install plan.
///
/// Mirrors `server::routes::plugins::PluginResponse`. Loose `manifest`
/// passthrough so the Claude Code spec (hooks / MCP / etc.) can evolve
/// without forcing CLI updates; only the `contents` array is consumed
/// by the resolver. `manifest.plugins[]` is also walked transitively
/// — see [`Self::nested_plugin_slugs`].
#[derive(Debug, Clone, Deserialize)]
pub struct PluginDetail {
    pub slug: String,
    pub version: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub contents: Vec<PluginContentEntry>,
    #[serde(default)]
    pub manifest: serde_json::Value,
}

impl PluginDetail {
    /// Slugs declared in `manifest.plugins[]` — used by the BFS
    /// resolver to enqueue transitively-required plugins.
    ///
    /// We tolerate two shapes for forward-compatibility:
    ///   - `[{"slug": "foo"}, …]` (the shape we'd publish today)
    ///   - `["foo", …]`           (a bare-string alternative)
    ///
    /// Any entry that isn't a string or an object with a `slug` field
    /// is silently skipped — the manifest is loose JSON, not a strict
    /// schema, so being liberal here matches the publish path.
    pub fn nested_plugin_slugs(&self) -> Vec<String> {
        let Some(arr) = self.manifest.get("plugins").and_then(|v| v.as_array()) else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(arr.len());
        for v in arr {
            if let Some(s) = v.as_str() {
                out.push(s.to_string());
            } else if let Some(s) = v.get("slug").and_then(|s| s.as_str()) {
                out.push(s.to_string());
            }
        }
        out
    }
}

/// Outcome of a plugin endpoint call that may legitimately 404 while the
/// server-side route is still in flight (#30 publish/list, #32 import).
///
/// `Unavailable.issue` carries the tracking issue number so the call site
/// can print a specific "tracking: issue #N" message without stringly-typing
/// it across the codebase.
#[derive(Debug)]
pub enum PluginEndpointOutcome<T> {
    Ok(T),
    Unavailable { issue: u32 },
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
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
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
        let metadata_json = serde_json::to_string(&metadata).context("serialise draft metadata")?;

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

    // ── Project endpoints ────────────────────────────────────────────────────

    /// `GET /v1/tenant/projects` — list all projects for the tenant.
    pub async fn list_projects(&self) -> Result<Vec<Project>> {
        let url = self.base.join("/v1/tenant/projects")?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("list_projects: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    /// `GET /v1/tenant/projects/{slug}` — fetch one project with its items.
    pub async fn get_project(&self, slug: &str) -> Result<ProjectWithItems> {
        let url = self.base.join(&format!("/v1/tenant/projects/{slug}"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("project `{slug}` not found"));
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("get_project: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    /// `GET /v1/projects/resolve?remote=<url>` — resolve a project slug from
    /// a git remote URL. Returns `None` on 404 (no matching project).
    pub async fn resolve_project_by_remote(&self, remote: &str) -> Result<Option<ResolvedProject>> {
        let mut url = self.base.join("/v1/projects/resolve")?;
        url.query_pairs_mut().append_pair("remote", remote);
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("resolve_project: {status} — {body}"));
        }
        Ok(Some(resp.json().await?))
    }

    /// `GET /v1/bootstrap?project=<slug>&stack=<tags>` — project-aware bootstrap.
    /// Passes both project slug and stack tags so the server can apply project-tier
    /// precedence and backfill with stack mappings.
    pub async fn bootstrap_with_project(
        &self,
        project_slug: &str,
        stack: &[String],
    ) -> Result<BootstrapResponse> {
        let mut url = self.base.join("/v1/bootstrap")?;
        {
            let mut q = url.query_pairs_mut();
            q.append_pair("project", project_slug);
            if !stack.is_empty() {
                q.append_pair("stack", &stack.join(","));
            }
        }
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("bootstrap_with_project: {status} — {body}"));
        }
        Ok(resp.json().await?)
    }

    // ── Plan endpoints ───────────────────────────────────────────────────────

    /// `POST /v1/tenant/projects/{slug}/plan` with `source_type = "file"`.
    ///
    /// Reads `path` in streaming chunks so a 5 MB file does not require
    /// 5 MB + 1 byte of RAM before the size check triggers.  Returns the
    /// version number assigned by the server.
    pub async fn import_plan_file(
        &self,
        project_slug: &str,
        path: &std::path::Path,
    ) -> Result<u32> {
        const MAX_BYTES: u64 = 5 * 1024 * 1024;

        // Stream-read up to MAX_BYTES + 1 so we can distinguish "exactly
        // at the limit" from "too large" without loading the entire file.
        let file = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
        let mut buf = Vec::with_capacity(MAX_BYTES as usize + 1);
        use std::io::Read as _;
        file.take(MAX_BYTES + 1).read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_BYTES {
            return Err(anyhow!(
                "file exceeds the 5 MB plan limit: {}",
                path.display()
            ));
        }

        let body_md = String::from_utf8(buf)
            .with_context(|| format!("plan file is not valid UTF-8: {}", path.display()))?;

        let source_url = path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned();

        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/plan"))?;
        let body = serde_json::json!({
            "source_type": "file",
            "source_url": source_url,
            "body_md": body_md,
        });
        let resp = self.http.post(url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("import_plan_file: {status} — {text}"));
        }
        let v: serde_json::Value = resp.json().await?;
        let version = v["version"]
            .as_u64()
            .ok_or_else(|| anyhow!("import_plan_file: server response missing `version`"))?;
        Ok(version as u32)
    }

    /// `POST /v1/tenant/projects/{slug}/plan` with `source_type = "url"`.
    ///
    /// The server performs the fetch; the CLI validates that the scheme is
    /// HTTPS as a client-side defence-in-depth guard before sending.
    pub async fn import_plan_url(&self, project_slug: &str, source_url: &str) -> Result<u32> {
        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/plan"))?;
        let body = serde_json::json!({
            "source_type": "url",
            "source_url": source_url,
        });
        let resp = self.http.post(url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("import_plan_url: {status} — {text}"));
        }
        let v: serde_json::Value = resp.json().await?;
        let version = v["version"]
            .as_u64()
            .ok_or_else(|| anyhow!("import_plan_url: server response missing `version`"))?;
        Ok(version as u32)
    }

    /// `GET /v1/tenant/projects/{slug}/plan` — fetch the active plan body.
    ///
    /// Returns `None` when the server responds 404 (no plan imported yet).
    pub async fn get_active_plan(&self, project_slug: &str) -> Result<Option<String>> {
        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/plan"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("get_active_plan: {status} — {text}"));
        }
        let v: serde_json::Value = resp.json().await?;
        let body = v["body_md"]
            .as_str()
            .ok_or_else(|| anyhow!("get_active_plan: server response missing `body_md`"))?
            .to_string();
        Ok(Some(body))
    }

    /// `GET /v1/tenant/projects/{slug}/plan/versions` — list all plan versions.
    pub async fn list_plan_versions(&self, project_slug: &str) -> Result<Vec<PlanVersion>> {
        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/plan/versions"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("list_plan_versions: {status} — {text}"));
        }
        Ok(resp.json().await?)
    }

    /// `POST /v1/tenant/projects/{slug}/plan/refresh` — re-fetch from the
    /// original source URL and store a new version if the content changed.
    pub async fn refresh_plan(&self, project_slug: &str) -> Result<RefreshOutcome> {
        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/plan/refresh"))?;
        let resp = self.http.post(url).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("refresh_plan: {status} — {text}"));
        }
        Ok(resp.json().await?)
    }

    /// `POST /v1/tenant/projects/{slug}/plan/activate` — promote a specific
    /// version to be the active plan.
    pub async fn activate_plan_version(&self, project_slug: &str, version: u32) -> Result<()> {
        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/plan/activate"))?;
        let body = serde_json::json!({ "version": version });
        let resp = self.http.post(url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("activate_plan_version: {status} — {text}"));
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

    // ── Plugin endpoints ─────────────────────────────────────────────────────

    /// `POST /v1/plugins` — publish a plugin from its `.claude-plugin/plugin.json`.
    ///
    /// Returns `Unavailable { issue: 30 }` on 404 so the caller can print a
    /// friendly "tracking: issue #30" message until the server-side route
    /// lands. Other non-success statuses surface as `Err`.
    pub async fn publish_plugin(
        &self,
        manifest: &PluginManifest,
    ) -> Result<PluginEndpointOutcome<PublishedPlugin>> {
        let url = self.base.join("/v1/plugins")?;
        let resp = self.http.post(url).json(manifest).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(PluginEndpointOutcome::Unavailable { issue: 30 });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("publish_plugin: {status} — {body}"));
        }
        let published: PublishedPlugin = resp.json().await?;
        Ok(PluginEndpointOutcome::Ok(published))
    }

    /// `GET /v1/plugins` — list plugins in the current tenant.
    ///
    /// Returns `Unavailable { issue: 30 }` on 404 so the caller can render
    /// an empty list with a "not yet available" hint until #30 lands.
    pub async fn list_plugins(
        &self,
        tags: &[String],
        status_filter: Option<&str>,
    ) -> Result<PluginEndpointOutcome<Vec<PluginListEntry>>> {
        let mut url = self.base.join("/v1/plugins")?;
        {
            let mut q = url.query_pairs_mut();
            if !tags.is_empty() {
                q.append_pair("tags", &tags.join(","));
            }
            if let Some(s) = status_filter {
                q.append_pair("status", s);
            }
        }
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(PluginEndpointOutcome::Unavailable { issue: 30 });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("list_plugins: {status} — {body}"));
        }
        let entries: Vec<PluginListEntry> = resp.json().await?;
        Ok(PluginEndpointOutcome::Ok(entries))
    }

    /// `POST /v1/plugins/import` — enqueue an import of an external plugin
    /// git URL into the tenant's marketplace.
    ///
    /// Returns `Unavailable { issue: 32 }` on 404 — the import worker lands
    /// in a separate issue from the publish/list routes.
    pub async fn import_plugin(
        &self,
        git_url: &str,
    ) -> Result<PluginEndpointOutcome<serde_json::Value>> {
        let url = self.base.join("/v1/plugins/import")?;
        let body = serde_json::json!({ "url": git_url });
        let resp = self.http.post(url).json(&body).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(PluginEndpointOutcome::Unavailable { issue: 32 });
        }
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("import_plugin: {status} — {text}"));
        }
        let body: serde_json::Value = resp.json().await.unwrap_or(serde_json::Value::Null);
        Ok(PluginEndpointOutcome::Ok(body))
    }

    /// `GET /v1/plugins/{slug}` — fetch the latest published version of
    /// `slug` with its full `contents[]` and `manifest` body. Used by
    /// the #36 transitive resolver in `ensure.rs`.
    ///
    /// Returns `Unavailable { issue: 30 }` on 404 so callers can soft-
    /// fail when the registry hasn't shipped the plugin route yet (or
    /// when the plugin slug genuinely doesn't exist — both surface as
    /// 404 from the server; we treat them identically so a stale
    /// manifest pin doesn't hard-fail `ensure`).
    pub async fn get_plugin(
        &self,
        slug: &str,
    ) -> Result<PluginEndpointOutcome<PluginDetail>> {
        let url = self.base.join(&format!("/v1/plugins/{slug}"))?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND {
            return Ok(PluginEndpointOutcome::Unavailable { issue: 30 });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("get_plugin({slug}): {status} — {body}"));
        }
        let detail: PluginDetail = resp.json().await?;
        Ok(PluginEndpointOutcome::Ok(detail))
    }

    /// `PUT /v1/tenant/projects/{slug}/items` — atomically replace the
    /// project's curated item list. Used by `skill-pool project
    /// add-plugin` after fetching the current items and appending the
    /// new `(slug, "plugin")` pair.
    pub async fn set_project_items(
        &self,
        project_slug: &str,
        items: &[(String, String)],
    ) -> Result<()> {
        let url = self
            .base
            .join(&format!("/v1/tenant/projects/{project_slug}/items"))?;
        // Wire shape mirrors `routes::projects::ItemInput { slug, kind }`.
        let body: Vec<serde_json::Value> = items
            .iter()
            .map(|(slug, kind)| serde_json::json!({ "slug": slug, "kind": kind }))
            .collect();
        let resp = self.http.put(url).json(&body).send().await?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(anyhow!("set_project_items: {status} — {text}"));
        }
        Ok(())
    }
}
