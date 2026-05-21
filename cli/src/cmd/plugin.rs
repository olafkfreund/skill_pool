//! `skill-pool plugin` subcommand family — publish, list, add, import,
//! marketplace-url. Mirrors the `project` / `plan` subcommand layout.
//!
//! Server-side routes for `publish` / `list` (#30) and `import` (#32) may
//! not be live yet; this module degrades gracefully on 404 — see the
//! `PluginEndpointOutcome::Unavailable` branches in each handler.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};

use crate::client::{Client, PluginEndpointOutcome, PluginManifest};
use crate::config::{Config, RegistryConfig};
use crate::manifest::{
    add_plugin_to_manifest, find_project_root, load_in, save_in, PluginAddOutcome,
};

/// Local plugin manifest path under a plugin source directory.
const PLUGIN_JSON_REL: &str = ".claude-plugin/plugin.json";

#[derive(Debug, clap::Subcommand)]
pub enum PluginAction {
    /// Validate a local plugin directory and publish it to the registry.
    ///
    /// The directory must contain `.claude-plugin/plugin.json` with the
    /// required fields (`name`, `version`) per the Claude Code spec.
    Publish {
        /// Path to a directory containing `.claude-plugin/plugin.json`.
        #[arg(value_name = "DIR")]
        dir: PathBuf,
    },

    /// List all plugins published in the current tenant.
    List {
        /// Filter to plugins tagged with ALL of these tags (comma-separated).
        #[arg(long, value_delimiter = ',')]
        tags: Vec<String>,
        /// Filter by plugin status.
        #[arg(long, value_parser = ["draft", "published", "archived"])]
        status: Option<String>,
        /// Emit one JSON object per line instead of a human table.
        #[arg(long)]
        json: bool,
    },

    /// Add a plugin reference to the workspace manifest (does not install).
    ///
    /// `<spec>` is `<slug>` or `<slug>@<version>`. Version defaults to `*`.
    /// Pure local: no registry validation — transitive resolution lands in #36.
    Add {
        /// e.g. `acme-toolkit` or `acme-toolkit@1.2.0`.
        #[arg(value_name = "SPEC")]
        spec: String,
    },

    /// Import an external plugin git URL into the tenant's marketplace.
    Import {
        /// HTTPS git URL of the external plugin repository.
        #[arg(value_name = "GIT_URL")]
        git_url: String,
    },

    /// Print the marketplace URL for `/plugin marketplace add <url>` in Claude Code.
    MarketplaceUrl,
}

/// Dispatcher. Returns an `ExitCode` so subcommands can distinguish
/// "couldn't do anything because the server isn't ready" (exit 2) from
/// "did what was asked" (exit 0) without raising an error up the chain.
pub async fn run(cfg: &Config, action: PluginAction) -> Result<ExitCode> {
    match action {
        PluginAction::Publish { dir } => publish(cfg, &dir).await,
        PluginAction::List { tags, status, json } => {
            list(cfg, &tags, status.as_deref(), json).await
        }
        PluginAction::Add { spec } => add(&spec).map(|_| ExitCode::SUCCESS),
        PluginAction::Import { git_url } => import(cfg, &git_url).await,
        PluginAction::MarketplaceUrl => marketplace_url(cfg).map(|_| ExitCode::SUCCESS),
    }
}

// ── publish ──────────────────────────────────────────────────────────────────

async fn publish(cfg: &Config, dir: &Path) -> Result<ExitCode> {
    let manifest_path = dir.join(PLUGIN_JSON_REL);
    if !manifest_path.exists() {
        bail!(
            "no `{PLUGIN_JSON_REL}` found in {} — is this a Claude Code plugin directory?",
            dir.display()
        );
    }

    let raw = std::fs::read_to_string(&manifest_path)
        .with_context(|| format!("read {}", manifest_path.display()))?;
    let manifest: PluginManifest = serde_json::from_str(&raw)
        .with_context(|| format!("parse {} as plugin.json", manifest_path.display()))?;

    validate_manifest(&manifest)?;
    println!(
        "  validated: {}@{} ({} bundled item{})",
        manifest.name,
        manifest.version,
        manifest.contents.len(),
        if manifest.contents.len() == 1 {
            ""
        } else {
            "s"
        },
    );

    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;

    match client.publish_plugin(&manifest).await? {
        PluginEndpointOutcome::Ok(published) => {
            println!(
                "  published: {}@{} [{}]",
                published.slug, published.version, published.status
            );
            Ok(ExitCode::SUCCESS)
        }
        PluginEndpointOutcome::Unavailable { issue } => {
            // Local validation succeeded; the server-side route just isn't
            // live yet. Exit 0 — we did the part we could.
            println!(
                "  note:      plugin publish endpoint not yet available on the registry \
                 (tracking: issue #{issue}). Validated `plugin.json` locally; \
                 nothing was published."
            );
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Local sanity checks before round-tripping to the server. Matches the
/// minimum-viable Claude Code plugin spec: `name` + `version` required,
/// no empty strings.
fn validate_manifest(manifest: &PluginManifest) -> Result<()> {
    if manifest.name.trim().is_empty() {
        bail!("plugin.json: `name` must not be empty");
    }
    if manifest.version.trim().is_empty() {
        bail!("plugin.json: `version` must not be empty");
    }
    for (i, content) in manifest.contents.iter().enumerate() {
        if content.slug.trim().is_empty() {
            bail!("plugin.json: contents[{i}].slug must not be empty");
        }
        if !matches!(content.kind.as_str(), "skill" | "agent" | "command") {
            bail!(
                "plugin.json: contents[{i}].kind must be one of skill|agent|command (got `{}`)",
                content.kind
            );
        }
    }
    Ok(())
}

// ── list ─────────────────────────────────────────────────────────────────────

async fn list(
    cfg: &Config,
    tags: &[String],
    status_filter: Option<&str>,
    json: bool,
) -> Result<ExitCode> {
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;

    let entries = match client.list_plugins(tags, status_filter).await? {
        PluginEndpointOutcome::Ok(v) => v,
        PluginEndpointOutcome::Unavailable { issue } => {
            if json {
                println!("[]");
            } else {
                println!(
                    "(plugin API not yet available on the registry — tracking: issue #{issue})"
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
    };

    if entries.is_empty() {
        if json {
            println!("[]");
        } else {
            println!("(no plugins published for tenant `{}`)", reg.tenant);
        }
        return Ok(ExitCode::SUCCESS);
    }

    if json {
        for e in &entries {
            println!("{}", serde_json::to_string(e)?);
        }
        return Ok(ExitCode::SUCCESS);
    }

    println!("{:<30}  {:<10}  {:<10}  NAME", "SLUG", "VERSION", "STATUS");
    println!("{}", "-".repeat(80));
    for e in &entries {
        println!(
            "{:<30}  {:<10}  {:<10}  {}",
            e.slug, e.version, e.status, e.name
        );
    }
    Ok(ExitCode::SUCCESS)
}

// ── add ──────────────────────────────────────────────────────────────────────

fn add(spec: &str) -> Result<()> {
    let (slug, version) = parse_spec(spec)?;
    let project_root = find_project_root().context("locate project root")?;
    let mut mf = load_in(&project_root).unwrap_or_default();

    match add_plugin_to_manifest(&mut mf, slug, version) {
        PluginAddOutcome::Inserted => {
            save_in(&project_root, &mf)?;
            println!("added: {slug}@{version} (manifest updated)");
        }
        PluginAddOutcome::AlreadyPresent => {
            println!("(already in manifest: {slug}@{version})");
        }
        PluginAddOutcome::Updated { previous_version } => {
            save_in(&project_root, &mf)?;
            println!("updated: {slug} {previous_version} → {version} (manifest updated)");
        }
    }
    Ok(())
}

/// Parse `foo` or `foo@1.2.0` into `(slug, version)`. Defaults `version`
/// to `*` (latest at install time) — matches the existing `SkillRef` default.
fn parse_spec(spec: &str) -> Result<(&str, &str)> {
    match spec.split_once('@') {
        Some((slug, version)) => {
            if slug.is_empty() {
                bail!("plugin add: slug must not be empty in `{spec}`");
            }
            if version.is_empty() {
                bail!("plugin add: version must not be empty after `@` in `{spec}`");
            }
            Ok((slug, version))
        }
        None => {
            if spec.is_empty() {
                bail!("plugin add: spec must not be empty");
            }
            Ok((spec, "*"))
        }
    }
}

// ── import ───────────────────────────────────────────────────────────────────

async fn import(cfg: &Config, git_url: &str) -> Result<ExitCode> {
    if !git_url.starts_with("https://") && !git_url.starts_with("git@") {
        bail!("plugin import: git URL must start with `https://` or `git@` (got `{git_url}`)");
    }
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;

    match client.import_plugin(git_url).await? {
        PluginEndpointOutcome::Ok(_) => {
            println!("queued: {git_url} (import job enqueued)");
            Ok(ExitCode::SUCCESS)
        }
        PluginEndpointOutcome::Unavailable { issue } => {
            // Unlike publish/list, there's nothing partial we can do here
            // without the server. Exit 2 (data unavailable) so CI scripts
            // can detect the not-yet-shipped path.
            println!("plugin import not yet available on the registry (tracking: issue #{issue}).");
            Ok(ExitCode::from(2))
        }
    }
}

// ── marketplace-url ──────────────────────────────────────────────────────────

fn marketplace_url(cfg: &Config) -> Result<()> {
    let reg = cfg.require_registry()?;
    let url = derive_marketplace_url(reg)?;
    println!("{url}");
    Ok(())
}

/// Derive the marketplace JSON URL from the configured registry.
///
/// Format: `https://<tenant>.<registry-host>/.claude-plugin/marketplace.json`
/// (per `docs/tenancy.md:39` — default origin is `https://{tenant}.skill-pool.example.com`).
///
/// Handles three host shapes:
///   1. Bare host (`registry.example.com`) → prefix `<tenant>.`.
///   2. Already-tenant-prefixed host (`acme.registry.example.com`) → don't double-prefix.
///   3. Localhost dev (`localhost`, `localhost:8080`, `127.0.0.1`) → still prefix
///      `<tenant>.` (matches the `localtest.me` dev pattern: `acme.localhost`
///      resolves via NSS / `/etc/hosts` or the `*.localtest.me` wildcard).
fn derive_marketplace_url(reg: &RegistryConfig) -> Result<String> {
    let parsed =
        url::Url::parse(&reg.url).with_context(|| format!("parse registry URL `{}`", reg.url))?;
    let scheme = parsed.scheme();
    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("registry URL `{}` has no host", reg.url))?;
    let port_suffix = parsed.port().map(|p| format!(":{p}")).unwrap_or_default();

    let tenant_prefix = format!("{}.", reg.tenant);
    let host_with_tenant = if host.starts_with(&tenant_prefix) {
        host.to_string()
    } else {
        format!("{tenant_prefix}{host}")
    };

    Ok(format!(
        "{scheme}://{host_with_tenant}{port_suffix}/.claude-plugin/marketplace.json"
    ))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(url: &str, tenant: &str) -> RegistryConfig {
        RegistryConfig {
            url: url.to_string(),
            tenant: tenant.to_string(),
            token: None,
        }
    }

    // ── parse_spec ────────────────────────────────────────────────────────────

    #[test]
    fn parse_spec_without_version_defaults_to_star() {
        let (slug, version) = parse_spec("acme-toolkit").unwrap();
        assert_eq!(slug, "acme-toolkit");
        assert_eq!(version, "*");
    }

    #[test]
    fn parse_spec_with_version() {
        let (slug, version) = parse_spec("acme-toolkit@1.2.0").unwrap();
        assert_eq!(slug, "acme-toolkit");
        assert_eq!(version, "1.2.0");
    }

    #[test]
    fn parse_spec_rejects_empty_slug() {
        assert!(parse_spec("@1.2.0").is_err());
    }

    #[test]
    fn parse_spec_rejects_empty_version() {
        assert!(parse_spec("foo@").is_err());
    }

    #[test]
    fn parse_spec_rejects_empty_string() {
        assert!(parse_spec("").is_err());
    }

    // ── derive_marketplace_url ────────────────────────────────────────────────

    #[test]
    fn marketplace_url_prefixes_tenant_on_bare_host() {
        let r = reg("https://registry.example.com", "acme");
        assert_eq!(
            derive_marketplace_url(&r).unwrap(),
            "https://acme.registry.example.com/.claude-plugin/marketplace.json"
        );
    }

    #[test]
    fn marketplace_url_does_not_double_prefix_already_tenanted_host() {
        let r = reg("https://acme.registry.example.com", "acme");
        assert_eq!(
            derive_marketplace_url(&r).unwrap(),
            "https://acme.registry.example.com/.claude-plugin/marketplace.json"
        );
    }

    #[test]
    fn marketplace_url_preserves_port_for_dev() {
        // localtest.me pattern: `acme.localhost:8080` resolves to 127.0.0.1
        // when using *.localtest.me or NSS-resolvable .localhost.
        let r = reg("http://localhost:8080", "acme");
        assert_eq!(
            derive_marketplace_url(&r).unwrap(),
            "http://acme.localhost:8080/.claude-plugin/marketplace.json"
        );
    }

    #[test]
    fn marketplace_url_handles_localtest_me() {
        let r = reg("http://acme.localtest.me:8080", "acme");
        assert_eq!(
            derive_marketplace_url(&r).unwrap(),
            "http://acme.localtest.me:8080/.claude-plugin/marketplace.json"
        );
    }

    #[test]
    fn marketplace_url_rejects_garbage_registry_url() {
        let r = reg("not a url", "acme");
        assert!(derive_marketplace_url(&r).is_err());
    }

    // ── validate_manifest ────────────────────────────────────────────────────

    fn mk_manifest(name: &str, version: &str) -> PluginManifest {
        PluginManifest {
            name: name.to_string(),
            version: version.to_string(),
            description: None,
            contents: vec![],
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn validate_manifest_accepts_minimal_valid_input() {
        assert!(validate_manifest(&mk_manifest("acme", "1.0.0")).is_ok());
    }

    #[test]
    fn validate_manifest_rejects_empty_name() {
        let err = validate_manifest(&mk_manifest("", "1.0.0")).unwrap_err();
        assert!(err.to_string().contains("`name`"));
    }

    #[test]
    fn validate_manifest_rejects_empty_version() {
        let err = validate_manifest(&mk_manifest("acme", "")).unwrap_err();
        assert!(err.to_string().contains("`version`"));
    }

    #[test]
    fn validate_manifest_rejects_unknown_content_kind() {
        let mut m = mk_manifest("acme", "1.0.0");
        m.contents.push(crate::client::PluginContentRef {
            kind: "monitor".into(),
            slug: "deploy-watch".into(),
            version: "1.0.0".into(),
        });
        let err = validate_manifest(&m).unwrap_err();
        assert!(err.to_string().contains("skill|agent|command"));
    }
}
