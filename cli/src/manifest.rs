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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProjectMeta {
    #[serde(default)]
    pub stack: Vec<String>,
    /// Override the tenant for this project (rare; usually inherits from CLI config).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tenant: Option<String>,
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
}
