use anyhow::{Context, Result};

use crate::config::Config;
use crate::detect;
use crate::manifest::{save_in, Manifest, ProjectMeta};

pub fn run(_cfg: &Config) -> Result<()> {
    let cwd = std::env::current_dir().context("get current dir")?;
    let detection = detect::detect(&cwd);

    let manifest = Manifest {
        project: ProjectMeta {
            stack: detection.stack.clone(),
            ..Default::default()
        },
        ..Default::default()
    };
    save_in(&cwd, &manifest)?;
    println!("wrote .skill-pool/manifest.toml");

    if detection.stack.is_empty() {
        println!("  stack: (none detected — add tags manually under [project])");
    } else {
        println!("  stack: {}", detection.stack.join(", "));
    }
    println!();
    println!("next:");
    println!("  skill-pool detect       # re-run detection (or --json for scripting)");
    println!("  skill-pool add <slug>   # add a skill explicitly");
    Ok(())
}
