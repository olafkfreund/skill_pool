//! `skill-pool bootstrap` — the one-keystroke "you're in a new project,
//! get the right skills" command. Ties detect + GET /v1/bootstrap +
//! manifest + ensure together.

use std::io::{IsTerminal, Read};

use anyhow::{anyhow, Context, Result};

use crate::client::Client;
use crate::cmd::ensure;
use crate::config::Config;
use crate::detect;
use crate::manifest::{find_project_root, load_in, manifest_path_in, save_in, Manifest, SkillRef};

pub async fn run(cfg: &Config, force_detect: bool, assume_yes: bool, dry_run: bool) -> Result<()> {
    // 1. Find or create project root + manifest.
    let project_root = find_project_root().context("locate project root")?;
    let mut mf = load_in(&project_root).unwrap_or_default();
    let manifest_existed = manifest_path_in(&project_root).exists();

    // 2. Determine stack tags.
    let stack: Vec<String> = if force_detect || mf.project.stack.is_empty() {
        let d = detect::detect_cached(&project_root)?;
        if d.stack.is_empty() {
            println!(
                "(no stack detected at {}; nothing to bootstrap)",
                project_root.display()
            );
            return Ok(());
        }
        mf.project.stack = d.stack.clone();
        d.stack
    } else {
        mf.project.stack.clone()
    };

    println!("stack: {}", stack.join(", "));

    // 3. Ask the server which skills are recommended for that stack.
    let reg = cfg.require_registry()?;
    let client = Client::new(reg)?;
    let recommended = client.bootstrap(&stack).await?.skills;

    if recommended.is_empty() {
        println!(
            "(no curated skills mapped to this stack on tenant `{}`)",
            reg.tenant
        );
        println!(
            "  set up some: skill-pool-server admin stack-map-set --tenant {} --stack <tag> --skill <slug>",
            reg.tenant
        );
        return Ok(());
    }

    // 4. Classify each recommendation against the existing manifest.
    let existing: std::collections::HashSet<&str> =
        mf.skills.iter().map(|s| s.slug.as_str()).collect();
    let plan = merge_plan(&recommended, &existing);

    if plan.to_add.is_empty() {
        println!(
            "all {} recommended skills already in manifest. Nothing to add.",
            recommended.len()
        );
        return Ok(());
    }

    println!();
    println!(
        "Recommended skills for this project ({}):",
        recommended.len()
    );
    for slug in &recommended {
        let marker = if plan.to_add.iter().any(|s| s == slug) {
            "+"
        } else {
            "·"
        };
        println!("  {marker} {slug}");
    }
    println!();
    println!("To add: {}", plan.to_add.len());

    // 5. Confirm.
    if !assume_yes {
        if !std::io::stdin().is_terminal() {
            return Err(anyhow!(
                "stdin is not a TTY; pass --yes to confirm non-interactively"
            ));
        }
        println!();
        print!("Add these to the manifest and install? [Y/n] ");
        use std::io::Write;
        std::io::stdout().flush().ok();
        let mut input = String::new();
        std::io::stdin().lock().read_to_string(&mut input).ok();
        // Only look at the first non-whitespace char.
        let answer = input
            .trim()
            .chars()
            .next()
            .unwrap_or('y')
            .to_ascii_lowercase();
        if answer == 'n' {
            println!("skipped.");
            return Ok(());
        }
    }

    // 6. Append + save.
    for slug in &plan.to_add {
        mf.skills.push(SkillRef {
            slug: slug.clone(),
            version: "*".to_string(),
            scope: "project".to_string(),
        });
    }

    if dry_run {
        println!();
        println!(
            "(dry-run; would add {} skills, would not call ensure)",
            plan.to_add.len()
        );
        return Ok(());
    }

    save_in(&project_root, &mf)?;
    println!(
        "added {} skills to {}",
        plan.to_add.len(),
        manifest_path_in(&project_root).display()
    );
    if !manifest_existed {
        println!("(new manifest created)");
    }

    // 7. Install them.
    println!();
    println!("Installing…");
    ensure::run(cfg).await
}

/// Pure-data helper: split recommended slugs into "to add" vs "already present".
pub(crate) fn merge_plan(
    recommended: &[String],
    existing: &std::collections::HashSet<&str>,
) -> Plan {
    let mut to_add = Vec::new();
    let mut already = Vec::new();
    for slug in recommended {
        if existing.contains(slug.as_str()) {
            already.push(slug.clone());
        } else {
            to_add.push(slug.clone());
        }
    }
    Plan { to_add, already }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub to_add: Vec<String>,
    #[allow(dead_code)] // surfaced indirectly via the · prefix in the print
    pub already: Vec<String>,
}

// Allow Manifest type used implicitly.
#[allow(dead_code)]
fn _typecheck(_: Manifest) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn classifies_to_add_vs_already() {
        let existing: HashSet<&str> = ["foo", "bar"].into_iter().collect();
        let recs: Vec<String> = ["foo", "baz", "bar", "qux"]
            .into_iter()
            .map(String::from)
            .collect();
        let plan = merge_plan(&recs, &existing);
        assert_eq!(plan.to_add, vec!["baz".to_string(), "qux".to_string()]);
        assert_eq!(plan.already, vec!["foo".to_string(), "bar".to_string()]);
    }

    #[test]
    fn all_already_present() {
        let existing: HashSet<&str> = ["foo"].into_iter().collect();
        let plan = merge_plan(&["foo".to_string()], &existing);
        assert!(plan.to_add.is_empty());
        assert_eq!(plan.already, vec!["foo".to_string()]);
    }

    #[test]
    fn all_new() {
        let existing: HashSet<&str> = HashSet::new();
        let plan = merge_plan(&["a".into(), "b".into()], &existing);
        assert_eq!(plan.to_add.len(), 2);
        assert!(plan.already.is_empty());
    }
}
