use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub registry: Option<RegistryConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    pub url: String,
    pub tenant: String,
    /// API token. Stored locally only; never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        let dirs = directories::ProjectDirs::from("dev", "calitii", "skill-pool")
            .ok_or_else(|| anyhow!("could not determine config dir"))?;
        Ok(dirs.config_dir().join("config.toml"))
    }

    pub fn load(explicit: Option<&Path>, registry_override: Option<&str>) -> Result<Self> {
        let path = match explicit {
            Some(p) => p.to_path_buf(),
            None => Self::default_path()?,
        };

        let mut cfg = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_context(|| format!("read config {}", path.display()))?;
            toml::from_str::<Config>(&raw)
                .with_context(|| format!("parse config {}", path.display()))?
        } else {
            Config::default()
        };

        if let Some(url) = registry_override {
            cfg.registry = Some(RegistryConfig {
                url: url.to_string(),
                tenant: cfg
                    .registry
                    .as_ref()
                    .map(|r| r.tenant.clone())
                    .unwrap_or_else(|| "default".into()),
                token: cfg.registry.as_ref().and_then(|r| r.token.clone()),
            });
        }

        Ok(cfg)
    }

    pub fn require_registry(&self) -> Result<&RegistryConfig> {
        self.registry.as_ref().ok_or_else(|| {
            anyhow!("no registry configured — run `skill-pool login --registry URL --tenant SLUG`")
        })
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::default_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| format!("write config {}", path.display()))?;
        Ok(())
    }
}
