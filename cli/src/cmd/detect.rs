use anyhow::Result;

use crate::detect;

pub fn run(json: bool, no_cache: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let d = if no_cache {
        detect::detect(&cwd)
    } else {
        detect::detect_cached(&cwd)?
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
        return Ok(());
    }

    if d.stack.is_empty() {
        println!("(no stack detected at {})", cwd.display());
        return Ok(());
    }
    println!("Detected stack ({} tags):", d.stack.len());
    for tag in &d.stack {
        println!("  - {tag}");
    }
    if !d.signals.is_empty() {
        println!();
        println!("Signals:");
        for s in &d.signals {
            println!("  • {s}");
        }
    }
    Ok(())
}
