use anyhow::{Context, Result};

use crate::config::Config;
use crate::detect;
use crate::manifest::{save_in, Manifest, ProjectMeta};

/// Arguments accepted by `skill-pool init`.
#[derive(Debug, clap::Args)]
pub struct InitArgs {
    /// Pin this workspace to a curator-defined project slug.
    /// Writes `manifest.project.slug` so that `skill-pool bootstrap`
    /// will query project-tier items first.
    #[arg(long, value_name = "SLUG")]
    pub project: Option<String>,
}

pub fn run(_cfg: &Config, args: &InitArgs) -> Result<()> {
    let cwd = std::env::current_dir().context("get current dir")?;
    let detection = detect::detect(&cwd);

    let manifest = Manifest {
        project: ProjectMeta {
            stack: detection.stack.clone(),
            slug: args.project.clone(),
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
    if let Some(slug) = &args.project {
        println!("  project.slug: {slug}");
    }
    println!();
    println!("next:");
    println!("  skill-pool detect       # re-run detection (or --json for scripting)");
    println!("  skill-pool bootstrap    # install curated skills for this project/stack");
    println!("  skill-pool add <slug>   # add a skill explicitly");
    Ok(())
}
