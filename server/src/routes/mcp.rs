//! MCP transport (Phase 5).
//!
//! A thin JSON-RPC 2.0 endpoint at `POST /v1/mcp` that exposes the
//! catalog as MCP tools, so a developer's Claude session can search
//! skills without leaving the conversation. Per the master plan,
//! MCP is a search adapter — not a replacement for REST + CLI.
//!
//! Methods implemented:
//!   - `initialize` — server capability handshake
//!   - `tools/list` — advertise the tools below
//!   - `tools/call` — dispatch
//!
//! Tools:
//!   - `search_skills(query?, tags?, semantic?, limit?, kind?)` — wraps
//!     the existing list endpoint. Returns a formatted text block listing
//!     matches, plus the raw JSON for tooling. `kind` defaults to
//!     `"skill"` to preserve pre-existing behaviour; pass `"agent"` or
//!     `"command"` to search the parallel catalog surfaces.
//!   - `get_skill(slug, kind?)` — returns the rendered `SKILL.md` body.
//!   - `install_skill(slug, kind?)` — returns the full bundle bytes as
//!     base64 + SHA-256 + slug + version + kind so the Claude session
//!     can extract and install the catalog item without leaving the
//!     chat. Capped at `MAX_BUNDLE_BYTES` (1 MiB); larger bundles
//!     surface a structured error that points the caller at
//!     `GET /v1/skills/{slug}/bundle.tar.gz`.
//!   - `get_project_plan(project_slug)` — fetch the active plan markdown
//!     for a project in this tenant. Returns the plan body plus version
//!     metadata. Tool-errors (isError: true) when the project has no
//!     imported plan yet or the project slug is not found.
//!
//! Auth: reuses the standard `AuthedCaller` extractor — operators set
//! `Authorization: Bearer <token>` and `X-Skill-Pool-Tenant: <slug>`
//! in their Claude MCP config.

use axum::extract::State;
use axum::Json;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

const PROTOCOL_VERSION: &str = "2025-03-26";
const SERVER_NAME: &str = "skill-pool";
const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");
const MAX_LIMIT: i64 = 50;
const DEFAULT_LIMIT: i64 = 10;
/// Hard cap on bundle bytes returned via `install_skill`. Bundles
/// over this size route the caller to the streaming HTTP endpoint
/// (`GET /v1/skills/{slug}/bundle.tar.gz`). Keeps a single JSON-RPC
/// response body bounded — base64 overhead is ~33%, so the wire
/// payload tops out around 1.4 MiB before headers.
const MAX_BUNDLE_BYTES: usize = 1024 * 1024; // 1 MiB

/// Catalog kinds exposed via MCP. Mirrors `routes::skills::VALID_KINDS`.
/// Kept local so this module is self-contained for the JSON-RPC layer.
const VALID_KINDS: &[&str] = &["skill", "agent", "command"];
const DEFAULT_KIND: &str = "skill";

/// Normalise an optional inbound kind string to one of the three
/// canonical values. Returns the static str so it can be bound straight
/// into SQL. Invalid input becomes an Err that the tool layer surfaces
/// as INVALID_PARAMS.
fn resolve_kind(raw: Option<&str>) -> Result<&'static str, String> {
    let v = raw.unwrap_or(DEFAULT_KIND).trim();
    match v {
        "skill" => Ok("skill"),
        "agent" => Ok("agent"),
        "command" => Ok("command"),
        other => Err(format!(
            "kind must be one of {VALID_KINDS:?}, got `{other}`"
        )),
    }
}

// ---------------------------------------------------------------------------
// JSON-RPC envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    /// Accepted for protocol completeness; we don't enforce a specific
    /// version because every MCP client sets "2.0" and a mismatch would
    /// confuse end users more than it would catch real bugs.
    #[serde(default)]
    #[allow(dead_code)]
    pub jsonrpc: String,
    pub method: String,
    /// `null` is valid (notification — but MCP uses `id`-bearing
    /// requests). We accept either.
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub params: Option<Value>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl JsonRpcResponse {
    fn ok(id: Option<Value>, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn err(id: Option<Value>, code: i32, message: impl Into<String>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.into(),
                data: None,
            }),
        }
    }
}

// Standard JSON-RPC 2.0 error codes.
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const INTERNAL_ERROR: i32 = -32603;

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

pub async fn handle(
    State(state): State<AppState>,
    caller: AuthedCaller,
    Json(req): Json<JsonRpcRequest>,
) -> AppResult<Json<JsonRpcResponse>> {
    let id = req.id.clone();
    let resp = match req.method.as_str() {
        "initialize" => initialize(id.clone()),
        "tools/list" => tools_list(id.clone()),
        "tools/call" => tools_call(&state, &caller, id.clone(), req.params).await,
        "ping" => JsonRpcResponse::ok(id.clone(), serde_json::json!({})),
        // Notifications/connected — clients may send these eagerly; ack
        // with empty success so we don't 404 spam the logs.
        m if m.starts_with("notifications/") => JsonRpcResponse::ok(id.clone(), Value::Null),
        other => JsonRpcResponse::err(
            id.clone(),
            METHOD_NOT_FOUND,
            format!("unknown method: {other}"),
        ),
    };
    Ok(Json(resp))
}

fn initialize(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "protocolVersion": PROTOCOL_VERSION,
            "capabilities": {
                "tools": { "listChanged": false }
            },
            "serverInfo": {
                "name": SERVER_NAME,
                "version": SERVER_VERSION,
            },
        }),
    )
}

fn tools_list(id: Option<Value>) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "tools": [
                {
                    "name": "search_skills",
                    "description":
                        "Search the team catalog for skills, agents, or commands. \
                         Returns matching slugs, versions, and descriptions. Use \
                         this to find a reusable team artefact before re-deriving \
                         a pattern from scratch. Pass `kind` to switch surfaces \
                         (defaults to `skill`).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "Substring matched against slug + description (keyword search)."
                            },
                            "tags": {
                                "type": "array",
                                "items": { "type": "string" },
                                "description": "All tags must be present on a result."
                            },
                            "semantic": {
                                "type": "string",
                                "description": "Rank by cosine similarity of description_embedding. Mutually exclusive with `query`; takes precedence if both supplied. Requires server build with --features fastembed."
                            },
                            "limit": {
                                "type": "integer",
                                "minimum": 1,
                                "maximum": 50,
                                "description": "Maximum results returned. Default 10."
                            },
                            "kind": {
                                "type": "string",
                                "enum": ["skill", "agent", "command"],
                                "default": "skill",
                                "description": "Catalog surface to search. `skill` (default), `agent`, or `command`."
                            }
                        },
                        "additionalProperties": false
                    }
                },
                {
                    "name": "get_skill",
                    "description":
                        "Fetch a catalog item's rendered SKILL.md (frontmatter + \
                         body). Use after `search_skills` to read the full \
                         contents. Pass `kind` to fetch agents or commands; \
                         defaults to `skill`.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["slug"],
                        "properties": {
                            "slug": {
                                "type": "string",
                                "description": "Catalog slug — exact match, no globbing."
                            },
                            "kind": {
                                "type": "string",
                                "enum": ["skill", "agent", "command"],
                                "default": "skill",
                                "description": "Catalog surface to fetch from. `skill` (default), `agent`, or `command`."
                            }
                        },
                        "additionalProperties": false
                    }
                },
                {
                    "name": "install_skill",
                    "description":
                        "Fetch the full catalog bundle (the `.tar.gz` that \
                         `skill-pool ensure` would download) as base64. The \
                         caller can decode and extract it into \
                         `.claude/skills/<slug>/` to install without leaving \
                         the chat. Returns slug, version, kind, sha256, and \
                         the base64-encoded bytes. Bundles over 1 MiB are \
                         refused — use `GET /v1/skills/{slug}/bundle.tar.gz` \
                         for those instead.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["slug"],
                        "properties": {
                            "slug": {
                                "type": "string",
                                "description": "Catalog slug — exact match, no globbing."
                            },
                            "kind": {
                                "type": "string",
                                "enum": ["skill", "agent", "command"],
                                "default": "skill",
                                "description": "Catalog surface to install from. `skill` (default), `agent`, or `command`."
                            }
                        },
                        "additionalProperties": false
                    }
                },
                {
                    "name": "get_project_plan",
                    "description":
                        "Fetch the active plan markdown for a project in this \
                         tenant. Returns the plan body and version metadata so \
                         you can answer questions about the project's current \
                         direction mid-session, without needing the developer \
                         to have run `skill-pool ensure` recently. Returns a \
                         tool error (isError: true) if the project has no \
                         imported plan yet or the project slug is not found.",
                    "inputSchema": {
                        "type": "object",
                        "required": ["project_slug"],
                        "properties": {
                            "project_slug": {
                                "type": "string",
                                "description": "Slug of the project (e.g. acme-billing-service)."
                            }
                        },
                        "additionalProperties": false
                    }
                }
            ]
        }),
    )
}

async fn tools_call(
    state: &AppState,
    caller: &AuthedCaller,
    id: Option<Value>,
    params: Option<Value>,
) -> JsonRpcResponse {
    let Some(params) = params else {
        return JsonRpcResponse::err(id, INVALID_PARAMS, "missing params");
    };
    let name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| Value::Object(Default::default()));

    match name {
        "search_skills" => match call_search(state, caller, args).await {
            Ok(v) => JsonRpcResponse::ok(id, v),
            Err(ToolError::NotFound(msg)) => tool_error_result(id, &msg),
            Err(ToolError::Invalid(msg)) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
            Err(ToolError::Internal(msg)) => JsonRpcResponse::err(id, INTERNAL_ERROR, msg),
        },
        "get_skill" => match call_get_skill(state, caller, args).await {
            Ok(v) => JsonRpcResponse::ok(id, v),
            Err(ToolError::NotFound(msg)) => tool_error_result(id, &msg),
            Err(ToolError::Invalid(msg)) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
            Err(ToolError::Internal(msg)) => JsonRpcResponse::err(id, INTERNAL_ERROR, msg),
        },
        "install_skill" => match call_install_skill(state, caller, args).await {
            Ok(v) => JsonRpcResponse::ok(id, v),
            Err(ToolError::NotFound(msg)) => tool_error_result(id, &msg),
            Err(ToolError::Invalid(msg)) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
            Err(ToolError::Internal(msg)) => JsonRpcResponse::err(id, INTERNAL_ERROR, msg),
        },
        "get_project_plan" => match call_get_project_plan(state, caller, args).await {
            Ok(v) => JsonRpcResponse::ok(id, v),
            Err(ToolError::NotFound(msg)) => tool_error_result(id, &msg),
            Err(ToolError::Invalid(msg)) => JsonRpcResponse::err(id, INVALID_PARAMS, msg),
            Err(ToolError::Internal(msg)) => JsonRpcResponse::err(id, INTERNAL_ERROR, msg),
        },
        other => JsonRpcResponse::err(id, METHOD_NOT_FOUND, format!("unknown tool: {other}")),
    }
}

#[derive(Debug)]
enum ToolError {
    Invalid(String),
    NotFound(String),
    Internal(String),
}

impl From<sqlx::Error> for ToolError {
    fn from(e: sqlx::Error) -> Self {
        ToolError::Internal(e.to_string())
    }
}

impl From<String> for ToolError {
    fn from(s: String) -> Self {
        ToolError::Internal(s)
    }
}

#[derive(Deserialize)]
struct SearchArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    tags: Option<Vec<String>>,
    #[serde(default)]
    semantic: Option<String>,
    #[serde(default)]
    limit: Option<i64>,
    /// Catalog surface to search. Defaults to `skill` so existing
    /// MCP clients that omit the field keep their behaviour.
    #[serde(default)]
    kind: Option<String>,
}

#[derive(sqlx::FromRow, Serialize)]
struct SearchRow {
    slug: String,
    version: String,
    description: String,
    tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    when_to_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    similarity: Option<f32>,
    created_at: DateTime<Utc>,
}

async fn call_search(
    state: &AppState,
    caller: &AuthedCaller,
    args: Value,
) -> Result<Value, ToolError> {
    let args: SearchArgs = serde_json::from_value(args)
        .map_err(|e| ToolError::Invalid(format!("invalid arguments: {e}")))?;
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let tag_list: Vec<String> = args.tags.unwrap_or_default();
    let kind = resolve_kind(args.kind.as_deref()).map_err(ToolError::Invalid)?;

    let rows: Vec<SearchRow> = if let Some(query_text) = args
        .semantic
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        // Semantic branch: mirror the REST endpoint's CTE + similarity ORDER.
        let embedding = state
            .embedder()
            .embed(query_text)
            .map_err(|e| ToolError::Internal(format!("embedder error: {e}")))?;
        let Some(v) = embedding else {
            return Err(ToolError::Invalid(
                "semantic search is not enabled on this server (no embedder configured)".into(),
            ));
        };
        let lit = crate::embedding::vector_to_pg_literal(&v);
        sqlx::query_as(
            "WITH latest AS ( \
               SELECT DISTINCT ON (slug) \
                 slug, version, description, when_to_use, tags, created_at, description_embedding \
               FROM skills \
               WHERE tenant_id = $1 \
                 AND kind = $5 \
                 AND status = 'published' \
                 AND ($2::text[] = '{}' OR tags @> $2) \
                 AND description_embedding IS NOT NULL \
               ORDER BY slug, created_at DESC \
             ) \
             SELECT slug, version, description, when_to_use, tags, \
                    (1 - (description_embedding <=> $3::text::vector))::real AS similarity, \
                    created_at \
             FROM latest \
             ORDER BY similarity DESC \
             LIMIT $4",
        )
        .bind(caller.tenant.tenant_id)
        .bind(&tag_list)
        .bind(lit)
        .bind(limit)
        .bind(kind)
        .fetch_all(state.db())
        .await?
    } else {
        let needle = args.query.as_deref().map(|s| format!("%{s}%"));
        sqlx::query_as(
            "SELECT DISTINCT ON (slug) \
                slug, version, description, when_to_use, tags, \
                NULL::real AS similarity, created_at \
             FROM skills \
             WHERE tenant_id = $1 \
               AND kind = $5 \
               AND status = 'published' \
               AND ($2::text IS NULL OR description ILIKE $2 OR slug ILIKE $2) \
               AND ($3::text[] = '{}' OR tags @> $3) \
             ORDER BY slug, created_at DESC \
             LIMIT $4",
        )
        .bind(caller.tenant.tenant_id)
        .bind(needle)
        .bind(&tag_list)
        .bind(limit)
        .bind(kind)
        .fetch_all(state.db())
        .await?
    };

    let text = render_search_text(&rows);
    let json = serde_json::to_value(&rows).unwrap_or(Value::Null);
    Ok(serde_json::json!({
        "content": [
            { "type": "text", "text": text },
            { "type": "text", "text": format!("```json\n{}\n```", serde_json::to_string_pretty(&json).unwrap_or_default()) }
        ],
        "isError": false
    }))
}

fn render_search_text(rows: &[SearchRow]) -> String {
    if rows.is_empty() {
        return "No matching skills.".to_string();
    }
    let mut out = format!("{} matching skill(s):\n\n", rows.len());
    for r in rows {
        out.push_str(&format!("- {}", r.slug));
        if let Some(sim) = r.similarity {
            out.push_str(&format!(" ({:.0}% match)", (sim * 100.0).clamp(0.0, 100.0)));
        }
        out.push_str(&format!(" — v{}\n", r.version));
        out.push_str(&format!("  {}\n", r.description));
        if let Some(when) = &r.when_to_use {
            out.push_str(&format!("  when: {when}\n"));
        }
        if !r.tags.is_empty() {
            out.push_str(&format!("  tags: {}\n", r.tags.join(", ")));
        }
    }
    out
}

#[derive(Deserialize)]
struct GetSkillArgs {
    slug: String,
    /// Catalog surface to fetch from. Defaults to `skill` so existing
    /// MCP clients that only pass `slug` keep their behaviour.
    #[serde(default)]
    kind: Option<String>,
}

async fn call_get_skill(
    state: &AppState,
    caller: &AuthedCaller,
    args: Value,
) -> Result<Value, ToolError> {
    let args: GetSkillArgs = serde_json::from_value(args)
        .map_err(|e| ToolError::Invalid(format!("invalid arguments: {e}")))?;
    let kind = resolve_kind(args.kind.as_deref()).map_err(ToolError::Invalid)?;
    let row: Option<(String,)> = sqlx::query_as(
        "SELECT bundle_uri FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&args.slug)
    .bind(kind)
    .fetch_optional(state.db())
    .await?;
    let Some((key,)) = row else {
        return Err(ToolError::NotFound(format!(
            "no published {} `{}` in tenant `{}`",
            kind, args.slug, caller.tenant.tenant_slug
        )));
    };

    let bytes = state
        .storage_for(&caller.tenant)
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?
        .read_bundle(&key)
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?;
    let md = read_skill_md(&bytes)
        .ok_or_else(|| ToolError::Internal("SKILL.md missing from bundle".into()))?;

    Ok(serde_json::json!({
        "content": [{ "type": "text", "text": md }],
        "isError": false
    }))
}

#[derive(Deserialize)]
struct InstallSkillArgs {
    slug: String,
    /// Catalog surface to install from. Defaults to `skill` so existing
    /// MCP clients that only pass `slug` keep their behaviour.
    #[serde(default)]
    kind: Option<String>,
}

async fn call_install_skill(
    state: &AppState,
    caller: &AuthedCaller,
    args: Value,
) -> Result<Value, ToolError> {
    use base64::Engine;

    let args: InstallSkillArgs = serde_json::from_value(args)
        .map_err(|e| ToolError::Invalid(format!("invalid arguments: {e}")))?;
    let kind = resolve_kind(args.kind.as_deref()).map_err(ToolError::Invalid)?;

    // Look up the latest published row for this slug+kind. We also pull
    // `version` and `bundle_sha256` so the MCP response can echo the
    // exact bytes' identity — clients verify the SHA after decoding to
    // catch base64 corruption.
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT version, bundle_uri, bundle_sha256 FROM skills \
         WHERE tenant_id = $1 AND slug = $2 AND kind = $3 AND status = 'published' \
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(caller.tenant.tenant_id)
    .bind(&args.slug)
    .bind(kind)
    .fetch_optional(state.db())
    .await?;
    let Some((version, key, sha256)) = row else {
        return Err(ToolError::NotFound(format!(
            "no published {} `{}` in tenant `{}`",
            kind, args.slug, caller.tenant.tenant_slug
        )));
    };

    let bytes = state
        .storage_for(&caller.tenant)
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?
        .read_bundle(&key)
        .await
        .map_err(|e| ToolError::Internal(e.to_string()))?;

    // Refuse oversize bundles rather than blow up the JSON-RPC envelope.
    // The HTTP endpoint streams without this cap, so the operator path
    // is to fall back to `GET /v1/skills/{slug}/bundle.tar.gz?kind=...`.
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(ToolError::Invalid(format!(
            "bundle for `{}` ({} bytes) exceeds the MCP response cap of {} bytes; \
             fetch via GET /v1/skills/{}/bundle.tar.gz?kind={} instead",
            args.slug,
            bytes.len(),
            MAX_BUNDLE_BYTES,
            args.slug,
            kind,
        )));
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let summary = format!(
        "Installable bundle for `{}` v{} [{}] — {} bytes (sha256={}).\n\
         Decode the base64 payload, write to a `.tar.gz`, and extract \
         into `.claude/skills/{}/` to install.",
        args.slug,
        version,
        kind,
        bytes.len(),
        sha256,
        args.slug,
    );

    // Structured second block: the bundle, plus the metadata a client
    // needs to verify the download (sha256) and place it on disk.
    let payload = serde_json::json!({
        "slug": args.slug,
        "version": version,
        "kind": kind,
        "sha256": sha256,
        "size_bytes": bytes.len() as i64,
        "bundle_base64": b64,
    });

    Ok(serde_json::json!({
        "content": [
            { "type": "text", "text": summary },
            { "type": "text", "text": format!("```json\n{}\n```", serde_json::to_string_pretty(&payload).unwrap_or_default()) }
        ],
        "isError": false
    }))
}

#[derive(Deserialize)]
struct GetProjectPlanArgs {
    project_slug: String,
}

async fn call_get_project_plan(
    state: &AppState,
    caller: &AuthedCaller,
    args: Value,
) -> Result<Value, ToolError> {
    let args: GetProjectPlanArgs = serde_json::from_value(args)
        .map_err(|e| ToolError::Invalid(format!("invalid arguments: {e}")))?;

    let result =
        crate::admin::get_active_plan(state.db(), &caller.tenant.tenant_slug, &args.project_slug)
            .await
            .map_err(|e| {
                // get_active_plan returns Err when the project slug doesn't exist.
                // Surface that as NotFound so the model gets isError: true rather
                // than a JSON-RPC internal error.
                let msg = e.to_string();
                if msg.contains("not found") {
                    ToolError::NotFound(msg)
                } else {
                    ToolError::Internal(msg)
                }
            })?;

    let Some(plan) = result else {
        return Err(ToolError::NotFound(format!(
            "project `{}` in tenant `{}` has no imported plan yet; \
             run `skill-pool ensure` or import a plan first",
            args.project_slug, caller.tenant.tenant_slug
        )));
    };

    let summary = format!(
        "Active plan for project `{}` (tenant `{}`), version {}.\n\
         Source: {} | SHA-256: {} | Imported: {}",
        args.project_slug,
        caller.tenant.tenant_slug,
        plan.version,
        plan.source_type,
        plan.body_sha256,
        plan.imported_at.format("%Y-%m-%dT%H:%M:%SZ"),
    );

    Ok(serde_json::json!({
        "content": [
            { "type": "text", "text": summary },
            { "type": "text", "text": plan.body_md }
        ],
        "isError": false
    }))
}

fn read_skill_md(bytes: &[u8]) -> Option<String> {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let gz = GzDecoder::new(bytes);
    let mut tar = tar::Archive::new(gz);
    for entry in tar.entries().ok()? {
        let mut entry = entry.ok()?;
        let path = entry.path().ok()?.to_path_buf();
        if path.to_string_lossy().trim_start_matches("./") == "SKILL.md" {
            let mut s = String::new();
            if entry.read_to_string(&mut s).is_ok() {
                return Some(s);
            }
        }
    }
    None
}

/// Build an MCP tool-error response — `isError: true` with the message
/// in a text content block. Used for the "skill not found" path that
/// MCP convention says should be a tool error (the model can recover),
/// not a JSON-RPC error (the protocol broke).
fn tool_error_result(id: Option<Value>, message: &str) -> JsonRpcResponse {
    JsonRpcResponse::ok(
        id,
        serde_json::json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    )
}

// Silence unused warning during initial bring-up; `AppError` is only
// needed if we promote tool errors to JSON-RPC errors later.
#[allow(dead_code)]
fn _swallow_unused(_: AppError) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initialize_advertises_tools_capability() {
        let r = initialize(Some(serde_json::json!(1)));
        let v = r.result.unwrap();
        assert_eq!(v["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(v["serverInfo"]["name"], SERVER_NAME);
        assert!(v["capabilities"]["tools"].is_object());
    }

    #[test]
    fn tools_list_advertises_all_tools() {
        let r = tools_list(Some(serde_json::json!(2)));
        let v = r.result.unwrap();
        let tools = v["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 4);
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"search_skills"));
        assert!(names.contains(&"get_skill"));
        assert!(names.contains(&"install_skill"));
        assert!(names.contains(&"get_project_plan"));
    }

    #[test]
    fn tools_list_advertises_kind_on_all_tools() {
        let r = tools_list(Some(serde_json::json!(2)));
        let v = r.result.unwrap();
        let tools = v["tools"].as_array().unwrap();
        // `get_project_plan` is project-scoped, not catalog-kind-scoped, so it
        // intentionally has no `kind` property. Skip it in this assertion.
        let catalog_tools: Vec<_> = tools
            .iter()
            .filter(|t| t["name"].as_str() != Some("get_project_plan"))
            .collect();
        for t in catalog_tools {
            let kind_schema = &t["inputSchema"]["properties"]["kind"];
            assert!(
                kind_schema.is_object(),
                "tool `{}` missing kind property",
                t["name"]
            );
            assert_eq!(kind_schema["default"], "skill");
            let variants = kind_schema["enum"].as_array().unwrap();
            let vs: Vec<&str> = variants.iter().map(|v| v.as_str().unwrap()).collect();
            assert_eq!(vs, vec!["skill", "agent", "command"]);
        }
    }

    #[test]
    fn get_project_plan_schema_has_required_project_slug() {
        let r = tools_list(Some(serde_json::json!(3)));
        let v = r.result.unwrap();
        let tools = v["tools"].as_array().unwrap();
        let tool = tools
            .iter()
            .find(|t| t["name"].as_str() == Some("get_project_plan"))
            .expect("get_project_plan tool not found");

        // Must have inputSchema of type object.
        assert_eq!(tool["inputSchema"]["type"], "object");

        // project_slug must be in required.
        let required = tool["inputSchema"]["required"].as_array().unwrap();
        let req_names: Vec<&str> = required.iter().map(|v| v.as_str().unwrap()).collect();
        assert!(
            req_names.contains(&"project_slug"),
            "project_slug not in required: {req_names:?}"
        );

        // project_slug property must be a string.
        assert_eq!(
            tool["inputSchema"]["properties"]["project_slug"]["type"],
            "string"
        );

        // No extra properties allowed.
        assert_eq!(tool["inputSchema"]["additionalProperties"], false);

        // No `kind` property on this tool.
        assert!(
            tool["inputSchema"]["properties"]["kind"].is_null(),
            "get_project_plan should not expose a kind property"
        );
    }

    #[test]
    fn tools_list_order_is_stable() {
        // Readers expect: search_skills, get_skill, install_skill, get_project_plan.
        let r = tools_list(Some(serde_json::json!(4)));
        let v = r.result.unwrap();
        let tools = v["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert_eq!(
            names,
            vec![
                "search_skills",
                "get_skill",
                "install_skill",
                "get_project_plan"
            ]
        );
    }

    #[test]
    fn resolve_kind_defaults_and_validates() {
        assert_eq!(resolve_kind(None).unwrap(), "skill");
        assert_eq!(resolve_kind(Some("skill")).unwrap(), "skill");
        assert_eq!(resolve_kind(Some("agent")).unwrap(), "agent");
        assert_eq!(resolve_kind(Some("command")).unwrap(), "command");
        assert!(resolve_kind(Some("plugin")).is_err());
        assert!(resolve_kind(Some("")).is_err());
    }

    #[test]
    fn render_search_text_handles_empty() {
        assert_eq!(render_search_text(&[]), "No matching skills.");
    }

    #[test]
    fn render_search_text_includes_similarity_when_set() {
        let rows = vec![SearchRow {
            slug: "foo".into(),
            version: "1.0.0".into(),
            description: "bar".into(),
            tags: vec!["test".into()],
            when_to_use: None,
            similarity: Some(0.94),
            created_at: chrono::Utc::now(),
        }];
        let s = render_search_text(&rows);
        assert!(s.contains("94% match"), "{s}");
        assert!(s.contains("foo"));
        assert!(s.contains("v1.0.0"));
    }
}
