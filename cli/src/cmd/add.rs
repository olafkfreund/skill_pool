use anyhow::Result;

use crate::config::Config;

pub async fn run(_cfg: &Config, _slug: &str) -> Result<()> {
    // TODO(#3): append to manifest then call ensure.
    anyhow::bail!("`add` is scaffolded but not yet implemented (issue #3)");
}
