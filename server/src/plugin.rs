//! Plugin domain types (Layer 3 — composes existing skills/agents/commands
//! into a single installable unit served via per-tenant marketplace.json
//! and the per-plugin dumb-HTTP git endpoint).
//!
//! Tables: `plugins`, `plugin_contents`, `plugin_marketplace_entries`
//! (migrations 0031 + 0032).
//!
//! Row-mapping in admin.rs follows the existing `Project` precedent —
//! manual `sqlx::query!` + struct construction — so this module has no
//! sqlx dependency and is reusable from the CLI crate.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A single row in the `plugins` table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Plugin {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub slug: String,
    pub version: String,
    pub name: String,
    pub description: Option<String>,
    pub manifest: serde_json::Value,
    pub status: PluginStatus,
    pub sourcing_mode: PluginSourcingMode,
    pub created_by: Option<Uuid>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Status flips that drive marketplace visibility without deleting rows.
/// Stored as TEXT + CHECK in the schema (see migration 0031).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginStatus {
    Draft,
    Published,
    Archived,
}

/// One bundled atomic item — references a published skill/agent/command
/// in the same tenant by its natural key `(slug, kind, version)`.
///
/// We intentionally don't FK to `skills.id` so cross-version content
/// swaps stay explicit and so manifest pins survive a republish.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginContent {
    pub plugin_id: Uuid,
    pub content_slug: String,
    /// One of: `"skill"`, `"agent"`, `"command"` (enforced by DB CHECK).
    pub content_kind: String,
    pub content_version: String,
    pub position: i32,
}

/// The canonical `.claude-plugin/plugin.json` body, validated at the API
/// layer in #2 and stored as JSONB in `plugins.manifest`.
///
/// Kept loose (free-form `extra`) so the Claude Code spec can evolve
/// (hooks / MCP servers / LSP servers / monitors / themes / settings)
/// without a schema change here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub contents: Vec<PluginContentRef>,
    /// Inline blobs the registry doesn't store as first-class rows but
    /// passes through verbatim into the generated git tree.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `{kind, slug, version}` triple inside a `PluginManifest.contents` entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginContentRef {
    pub kind: String,
    pub slug: String,
    pub version: String,
}

/// How the plugin's source tree is sourced for clients to clone.
///
/// Stored as TEXT in `plugins.sourcing_mode` with paired
/// `external_git_url` / `upstream_url` columns. We don't use a pg enum
/// because the variants carry data; the row-mapper in admin.rs (landing
/// in #2) reads the discriminator + the appropriate URL column.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum PluginSourcingMode {
    /// Authored in the registry; served from skill-pool's own git endpoint.
    Internal,
    /// External git URL; marketplace.json points clients straight at it.
    External { git_url: String },
    /// Mirror of an external upstream — cloned into skill-pool storage
    /// and served from the local git endpoint (air-gapped tenants).
    Mirror { upstream_url: String },
}
