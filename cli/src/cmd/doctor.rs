use anyhow::Result;

use crate::config::Config;

pub fn run(cfg: &Config) -> Result<()> {
    println!("skill-pool doctor — v{}", env!("CARGO_PKG_VERSION"));
    match &cfg.registry {
        Some(r) => {
            println!("  registry: {}", r.url);
            println!("  tenant:   {}", r.tenant);
            println!(
                "  token:    {}",
                if r.token.is_some() { "set" } else { "MISSING" }
            );
        }
        None => println!("  registry: (not configured — run `skill-pool login`)"),
    }
    // TODO(#3): enumerate loaded skills, dangling symlinks, manifest drift.
    Ok(())
}
