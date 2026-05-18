use anyhow::Result;

use crate::client::Client;
use crate::config::Config;

pub async fn run(cfg: &Config, _query: &str) -> Result<()> {
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let health = client.healthz().await?;
    println!("registry reachable: {health}");
    // TODO(#3): call /v1/skills?query=...; format as table.
    anyhow::bail!("`search` is scaffolded but not yet implemented (issue #3)");
}
