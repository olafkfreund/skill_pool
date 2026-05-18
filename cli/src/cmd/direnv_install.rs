use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};

const LIBRARY_FILENAME: &str = "use_skill_pool.sh";

/// Embedded at compile time so the binary is self-contained — `cargo install`
/// users get the library without needing to fetch the repo.
const LIBRARY_BYTES: &[u8] = include_bytes!("../../../direnv/use_skill_pool.sh");

pub fn run(force: bool) -> Result<()> {
    let target = install_target()?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("mkdir -p {}", parent.display()))?;
    }

    if target.exists() && !force {
        let existing = std::fs::read(&target).ok();
        if existing.as_deref() == Some(LIBRARY_BYTES) {
            println!("already installed (up to date): {}", target.display());
        } else {
            println!("a different version exists at {}", target.display());
            println!("  use --force to overwrite, or diff manually first");
            return Ok(());
        }
    } else {
        std::fs::write(&target, LIBRARY_BYTES)
            .with_context(|| format!("write {}", target.display()))?;
        println!("installed direnv library: {}", target.display());
    }

    println!();
    println!("In each project's .envrc, add:");
    println!("  use skill_pool                # silent ensure on shell entry");
    println!("  use skill_pool bootstrap      # detect + recommend + install first-time");
    println!();
    println!("Then `direnv allow` and you're set.");
    Ok(())
}

fn install_target() -> Result<PathBuf> {
    let xdg = std::env::var("XDG_CONFIG_HOME").ok();
    let dir = if let Some(x) = xdg.filter(|s| !s.is_empty()) {
        PathBuf::from(x).join("direnv").join("lib")
    } else {
        let home = std::env::var("HOME")
            .map_err(|_| anyhow!("HOME not set; can't determine direnv lib path"))?;
        PathBuf::from(home).join(".config/direnv/lib")
    };
    Ok(dir.join(LIBRARY_FILENAME))
}
