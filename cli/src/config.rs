use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    pub registry: Option<RegistryConfig>,
    /// Optional URL of the web portal. When set, the capturer attaches a
    /// `web_url/drafts/<id>` deep-link to the desktop notification so the
    /// developer can click straight into the curator inbox. Not required
    /// for the registry API; pure UX sugar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub web_url: Option<String>,
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

    fn resolve_path(explicit: Option<&Path>) -> Result<PathBuf> {
        match explicit {
            Some(p) => Ok(p.to_path_buf()),
            None => Self::default_path(),
        }
    }

    pub fn load(explicit: Option<&Path>, registry_override: Option<&str>) -> Result<Self> {
        let path = Self::resolve_path(explicit)?;

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

    /// Persist the config. Honors the `--config` / `SKILL_POOL_CONFIG`
    /// override path so `save` is symmetric with `load` — a CLI invoked
    /// with `--config /tmp/x.toml login …` writes to `/tmp/x.toml` and
    /// later `--config /tmp/x.toml search …` reads back the same file.
    pub fn save(&self, explicit: Option<&Path>) -> Result<()> {
        let path = Self::resolve_path(explicit)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = toml::to_string_pretty(self)?;
        std::fs::write(&path, raw).with_context(|| format!("write config {}", path.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn save_honors_explicit_path_for_round_trip_with_load() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("custom.toml");

        let cfg = Config {
            registry: Some(RegistryConfig {
                url: "https://example.test".into(),
                tenant: "acme".into(),
                token: Some("sp_test_token".into()),
            }),
            web_url: None,
        };
        cfg.save(Some(&path)).expect("save honors explicit path");
        assert!(path.exists(), "save wrote to the override path");

        let loaded = Config::load(Some(&path), None).expect("load honors explicit path");
        let reg = loaded.registry.expect("registry persisted");
        assert_eq!(reg.url, "https://example.test");
        assert_eq!(reg.tenant, "acme");
        assert_eq!(reg.token.as_deref(), Some("sp_test_token"));
    }
}
