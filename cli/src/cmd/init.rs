use anyhow::{Context, Result};

use crate::config::Config;
use crate::manifest::{save_in, Manifest, ProjectMeta};

pub fn run(_cfg: &Config) -> Result<()> {
    let cwd = std::env::current_dir().context("get current dir")?;
    let manifest = Manifest {
        project: ProjectMeta::default(),
        ..Default::default()
    };
    save_in(&cwd, &manifest)?;
    println!("wrote .skill-pool/manifest.toml");
    println!("next: run `skill-pool detect` (Phase 3) or `skill-pool add <slug>` to populate it");
    Ok(())
}
