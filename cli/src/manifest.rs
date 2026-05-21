use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

pub const MANIFEST_REL: &str = ".skill-pool/manifest.toml";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub project: ProjectMeta,
    #[serde(default)]
    pub skills: Vec<SkillRef>,
    #[serde(default)]
    pub agents: Vec<SkillRef>,
    #[serde(default)]
    pub commands: Vec<SkillRef>,
    /// Plugin pins. Full transitive resolution (fetching each plugin's
    /// contained skills/agents/commands and merging them into the install
    /// plan) lands in #36 — this issue only wires the field through so
    /// manifests round-trip cleanly.
    #[serde(default)]
    pub plugins: Vec<PluginRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectMeta {
    #[serde(default)]
    pub stack: Vec<String>,
    /// Override the tenant for this project (rare; usually inherits from CLI config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
    /// Optional curator-assigned project identifier.
    /// Resolved server-side via /v1/projects/resolve or set manually
    /// by `skill-pool project link <slug>` / `skill-pool init --project <slug>`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Cached git remote URL (auto-discovered on first bootstrap).
    /// Lets future bootstrap calls skip the `git remote get-url` shell-out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRef {
    pub slug: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// "project" symlinks into ./.claude/skills/; "personal" into ~/.claude/skills/.
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_version() -> String {
    "*".into()
}
fn default_scope() -> String {
    "project".into()
}

/// A plugin pin in `manifest.plugins`. Same shape as `SkillRef` so the
/// TOML writer emits a uniform `[[plugins]]` array block. Distinct type
/// so the install-plan code can dispatch on plugin vs. atomic content
/// (and so the parallel-array `kind` dispatch in `ensure.rs` doesn't
/// accidentally treat a plugin slug as a skill slug).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRef {
    pub slug: String,
    #[serde(default = "default_version")]
    pub version: String,
    /// Symlink scope when the plugin's contained items are eventually
    /// installed (#36). Today: parsed and round-tripped, otherwise inert.
    #[serde(default = "default_scope")]
    pub scope: String,
}

pub fn manifest_path_in(dir: &Path) -> PathBuf {
    dir.join(MANIFEST_REL)
}

#[allow(dead_code)] // consumed by ensure/add commands once implemented (#3)
pub fn load_in(dir: &Path) -> Result<Manifest> {
    let path = manifest_path_in(dir);
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read manifest {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("parse manifest {}", path.display()))
}

pub fn save_in(dir: &Path, manifest: &Manifest) -> Result<()> {
    let path = manifest_path_in(dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let raw = toml::to_string_pretty(manifest)?;
    std::fs::write(&path, raw).with_context(|| format!("write manifest {}", path.display()))
}

#[allow(dead_code)] // consumed by Phase 3 bootstrap (#5)
pub fn find_project_root() -> Result<PathBuf> {
    let mut here = std::env::current_dir()?;
    loop {
        if here.join(MANIFEST_REL).exists() || here.join(".git").exists() {
            return Ok(here);
        }
        if !here.pop() {
            return Err(anyhow!("could not find a project root from current dir"));
        }
    }
}

/// Append a catalog item into the manifest array selected by `kind`.
/// Returns `true` when the entry was newly inserted and `false` when an
/// entry with the same slug was already present (in any version/scope).
///
/// Kind dispatch matches the catalog kinds the server understands:
///   - `skill`   → `manifest.skills`
///   - `agent`   → `manifest.agents`
///   - `command` → `manifest.commands`
///
/// New entries default to `version="*"` (latest at install time) and
/// `scope="project"` to match the historical `add` behaviour.
pub fn add_to_manifest(manifest: &mut Manifest, slug: &str, kind: &str) -> Result<bool> {
    let bucket: &mut Vec<SkillRef> = match kind {
        "skill" => &mut manifest.skills,
        "agent" => &mut manifest.agents,
        "command" => &mut manifest.commands,
        other => {
            return Err(anyhow!(
                "unknown kind `{other}`; expected skill|agent|command"
            ))
        }
    };
    if bucket.iter().any(|s| s.slug == slug) {
        return Ok(false);
    }
    bucket.push(SkillRef {
        slug: slug.to_string(),
        version: default_version(),
        scope: default_scope(),
    });
    Ok(true)
}

/// Outcome of adding a plugin pin. Distinguishes the three user-visible
/// states so the calling subcommand can print the right message.
#[derive(Debug, PartialEq, Eq)]
pub enum PluginAddOutcome {
    /// A new `[[plugins]]` entry was appended.
    Inserted,
    /// An entry with the same `(slug, version)` was already present — no-op.
    AlreadyPresent,
    /// An entry for `slug` existed at a different version; the version was
    /// updated in place. Carries the prior version for the user message.
    Updated { previous_version: String },
}

/// Add (or update) a `[[plugins]]` entry by slug. See `PluginAddOutcome`
/// for the three branches: insert / no-op / update-in-place.
///
/// Distinct from `add_to_manifest` because plugins pin an explicit
/// version (`plugin add foo@1.2.0`), so re-adding a different version is
/// a meaningful update, whereas re-adding a skill at `version = "*"` is
/// always a no-op.
pub fn add_plugin_to_manifest(
    manifest: &mut Manifest,
    slug: &str,
    version: &str,
) -> PluginAddOutcome {
    if let Some(existing) = manifest.plugins.iter_mut().find(|p| p.slug == slug) {
        if existing.version == version {
            return PluginAddOutcome::AlreadyPresent;
        }
        let previous_version = std::mem::replace(&mut existing.version, version.to_string());
        return PluginAddOutcome::Updated { previous_version };
    }
    manifest.plugins.push(PluginRef {
        slug: slug.to_string(),
        version: version.to_string(),
        scope: default_scope(),
    });
    PluginAddOutcome::Inserted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_to_manifest_routes_by_kind() {
        let mut mf = Manifest::default();
        assert!(add_to_manifest(&mut mf, "foo", "skill").unwrap());
        assert!(add_to_manifest(&mut mf, "bar", "agent").unwrap());
        assert!(add_to_manifest(&mut mf, "baz", "command").unwrap());
        assert_eq!(mf.skills.len(), 1);
        assert_eq!(mf.agents.len(), 1);
        assert_eq!(mf.commands.len(), 1);
        assert_eq!(mf.skills[0].slug, "foo");
        assert_eq!(mf.agents[0].slug, "bar");
        assert_eq!(mf.commands[0].slug, "baz");
    }

    #[test]
    fn add_to_manifest_dedups_within_kind() {
        let mut mf = Manifest::default();
        assert!(add_to_manifest(&mut mf, "foo", "skill").unwrap());
        // Re-adding the same slug as a skill is a no-op.
        assert!(!add_to_manifest(&mut mf, "foo", "skill").unwrap());
        assert_eq!(mf.skills.len(), 1);
    }

    #[test]
    fn add_to_manifest_rejects_unknown_kind() {
        let mut mf = Manifest::default();
        assert!(add_to_manifest(&mut mf, "foo", "plugin").is_err());
    }

    #[test]
    fn project_meta_round_trip_with_slug_and_remote() {
        let toml_in = r#"
[project]
stack = ["rust", "axum"]
slug = "acme-billing-service"
remote = "git@github.com:acme/billing.git"

[[skills]]
slug = "code-review-mastery"
version = "*"
scope = "project"
"#;
        let mf: Manifest = toml::from_str(toml_in).expect("parse");
        assert_eq!(mf.project.slug.as_deref(), Some("acme-billing-service"));
        assert_eq!(
            mf.project.remote.as_deref(),
            Some("git@github.com:acme/billing.git")
        );
        assert_eq!(mf.project.stack, vec!["rust", "axum"]);

        // Re-serialise and re-parse: slug + remote survive the round-trip.
        let toml_out = toml::to_string_pretty(&mf).expect("serialize");
        let mf2: Manifest = toml::from_str(&toml_out).expect("re-parse");
        assert_eq!(mf2.project.slug, mf.project.slug);
        assert_eq!(mf2.project.remote, mf.project.remote);
    }

    #[test]
    fn project_meta_round_trip_without_slug() {
        // A legacy manifest with no [project].slug or [project].remote
        // must parse successfully with both fields defaulting to None.
        let toml_in = r#"
[project]
stack = ["python"]

[[skills]]
slug = "clean-code"
version = "1.0.0"
scope = "project"
"#;
        let mf: Manifest = toml::from_str(toml_in).expect("parse legacy manifest");
        assert!(mf.project.slug.is_none(), "slug should be None");
        assert!(mf.project.remote.is_none(), "remote should be None");

        // When serialised, [project] must not have a `slug =` or `remote =` key.
        let toml_out = toml::to_string_pretty(&mf).expect("serialize");
        // "slug" can appear inside [[skills]] entries; we specifically check
        // that the [project] section does not emit a `slug` key of its own.
        // The simplest way: re-parse and confirm the fields are still None.
        let mf2: Manifest = toml::from_str(&toml_out).expect("re-parse");
        assert!(
            mf2.project.slug.is_none(),
            "project.slug must stay None after round-trip"
        );
        assert!(
            mf2.project.remote.is_none(),
            "project.remote must stay None after round-trip"
        );
        // Also confirm the [project] block itself has no `slug =` line.
        // We look for `slug =` appearing *before* the first `[[skills]]` line.
        let project_section_end = toml_out.find("[[skills]]").unwrap_or(toml_out.len());
        let project_section = &toml_out[..project_section_end];
        assert!(
            !project_section.contains("\nslug ="),
            "project section must not contain 'slug =': {project_section}"
        );
        assert!(
            !project_section.contains("\nremote ="),
            "project section must not contain 'remote =': {project_section}"
        );
    }

    // ── plugin pin helpers ────────────────────────────────────────────────────

    #[test]
    fn add_plugin_to_manifest_inserts_new_entry() {
        let mut mf = Manifest::default();
        let outcome = add_plugin_to_manifest(&mut mf, "acme-toolkit", "1.2.0");
        assert_eq!(outcome, PluginAddOutcome::Inserted);
        assert_eq!(mf.plugins.len(), 1);
        assert_eq!(mf.plugins[0].slug, "acme-toolkit");
        assert_eq!(mf.plugins[0].version, "1.2.0");
        assert_eq!(mf.plugins[0].scope, "project");
    }

    #[test]
    fn add_plugin_to_manifest_dedups_same_version() {
        let mut mf = Manifest::default();
        add_plugin_to_manifest(&mut mf, "acme-toolkit", "1.2.0");
        let outcome = add_plugin_to_manifest(&mut mf, "acme-toolkit", "1.2.0");
        assert_eq!(outcome, PluginAddOutcome::AlreadyPresent);
        assert_eq!(mf.plugins.len(), 1, "no second entry appended");
    }

    #[test]
    fn add_plugin_to_manifest_updates_version_in_place() {
        let mut mf = Manifest::default();
        add_plugin_to_manifest(&mut mf, "acme-toolkit", "1.2.0");
        let outcome = add_plugin_to_manifest(&mut mf, "acme-toolkit", "1.3.0");
        assert_eq!(
            outcome,
            PluginAddOutcome::Updated {
                previous_version: "1.2.0".to_string(),
            }
        );
        assert_eq!(mf.plugins.len(), 1);
        assert_eq!(mf.plugins[0].version, "1.3.0");
    }

    #[test]
    fn plugin_manifest_round_trips() {
        // Manifest with skills + plugins must parse, re-serialise, and
        // re-parse to the same logical content. Catches accidental
        // breakage of the new `[[plugins]]` array block.
        let toml_in = r#"
[project]
stack = ["rust"]

[[skills]]
slug = "code-review-mastery"
version = "*"
scope = "project"

[[plugins]]
slug = "acme-toolkit"
version = "1.2.0"
scope = "project"
"#;
        let mf: Manifest = toml::from_str(toml_in).expect("parse");
        assert_eq!(mf.plugins.len(), 1);
        assert_eq!(mf.plugins[0].slug, "acme-toolkit");
        assert_eq!(mf.plugins[0].version, "1.2.0");

        let toml_out = toml::to_string_pretty(&mf).expect("serialize");
        let mf2: Manifest = toml::from_str(&toml_out).expect("re-parse");
        assert_eq!(mf2.plugins.len(), 1);
        assert_eq!(mf2.plugins[0].slug, mf.plugins[0].slug);
        assert_eq!(mf2.plugins[0].version, mf.plugins[0].version);
        assert_eq!(mf2.plugins[0].scope, mf.plugins[0].scope);
    }

    #[test]
    fn legacy_manifest_without_plugins_parses() {
        // Pre-#33 manifests have no [[plugins]] block; they must still
        // parse cleanly with `plugins` defaulting to empty.
        let toml_in = r#"
[project]
stack = ["python"]

[[skills]]
slug = "clean-code"
version = "1.0.0"
scope = "project"
"#;
        let mf: Manifest = toml::from_str(toml_in).expect("parse legacy manifest");
        assert!(mf.plugins.is_empty(), "plugins should default to empty");
    }
}
