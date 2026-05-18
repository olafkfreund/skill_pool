use anyhow::Result;

use crate::config::Config;

pub async fn run(_cfg: &Config) -> Result<()> {
    // TODO(#3): read manifest, fetch missing bundles from registry, symlink into
    // .claude/skills/, prune stale links. Use the same symlink semantics as
    // scripts/install.sh.
    anyhow::bail!("`ensure` is scaffolded but not yet implemented (issue #3)");
}
