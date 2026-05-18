use anyhow::Result;

use crate::config::{Config, RegistryConfig};

pub async fn run(cfg: &Config, registry: &str, tenant: &str) -> Result<()> {
    let token = rpassword::prompt_password("API token: ")?;
    if token.trim().is_empty() {
        anyhow::bail!("token must not be empty");
    }

    let mut new_cfg = cfg.clone();
    new_cfg.registry = Some(RegistryConfig {
        url: registry.to_string(),
        tenant: tenant.to_string(),
        token: Some(token),
    });
    new_cfg.save()?;
    println!("saved credentials for tenant `{tenant}` at {registry}");
    Ok(())
}
