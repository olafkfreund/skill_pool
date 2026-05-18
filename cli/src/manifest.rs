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
