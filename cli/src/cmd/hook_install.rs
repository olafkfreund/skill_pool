//! `skill-pool hook-install` — wires a Claude Code SessionStart hook into
//! `<project>/.claude/settings.json` so every Claude session re-runs
//! `skill-pool ensure --quiet` on startup.
//!
//! Complements the direnv path: direnv runs on shell entry; the
//! SessionStart hook covers users who skip direnv or open Claude
//! directly without a shell `cd`.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::manifest::find_project_root;

const HOOK_COMMAND: &str = "skill-pool ensure --quiet";
const HOOK_TIMEOUT: u64 = 30;

pub fn run(remove: bool, print_only: bool) -> Result<()> {
    let project_root = find_project_root().context("locate project root")?;
    let settings_path = project_root.join(".claude").join("settings.json");

    let mut settings = if settings_path.exists() {
        let raw = std::fs::read_to_string(&settings_path)
            .with_context(|| format!("read {}", settings_path.display()))?;
        if raw.trim().is_empty() {
            json!({})
        } else {
            serde_json::from_str::<Value>(&raw).with_context(|| {
                format!("parse {} as JSON — fix it manually first", settings_path.display())
            })?
        }
    } else {
        json!({})
    };

    let changed = if remove {
        remove_hook(&mut settings)
    } else {
        add_hook(&mut settings)
    };

    if print_only {
        println!("{}", serde_json::to_string_pretty(&settings)?);
        return Ok(());
    }

    if !changed {
        if remove {
            println!("(no skill-pool SessionStart hook found in {})", settings_path.display());
        } else {
            println!("(skill-pool SessionStart hook already present in {})", settings_path.display());
        }
        return Ok(());
    }

    if let Some(parent) = settings_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("mkdir -p {}", parent.display()))?;
    }
    let mut content = serde_json::to_string_pretty(&settings)?;
    content.push('\n');
    std::fs::write(&settings_path, content)
        .with_context(|| format!("write {}", settings_path.display()))?;

    if remove {
        println!("removed skill-pool SessionStart hook from {}", settings_path.display());
    } else {
        println!("installed skill-pool SessionStart hook in {}", settings_path.display());
        println!();
        println!("Next time Claude opens here, it will run:");
        println!("  {HOOK_COMMAND}");
    }
    Ok(())
}

/// Merge our SessionStart entry into `settings.hooks.SessionStart`.
/// Returns true if the document changed.
pub(crate) fn add_hook(settings: &mut Value) -> bool {
    let hooks = settings
        .as_object_mut()
        .and_then(|o| {
            o.entry("hooks".to_string()).or_insert(json!({}));
            o.get_mut("hooks")
        })
        .and_then(|h| h.as_object_mut())
        .expect("settings root is an object after construction");

    let session_start = hooks
        .entry("SessionStart".to_string())
        .or_insert(json!([]));
    let arr = match session_start.as_array_mut() {
        Some(a) => a,
        None => return false,
    };

    if has_our_hook(arr) {
        return false;
    }

    arr.push(json!({
        "matcher": "*",
        "hooks": [
            { "type": "command", "command": HOOK_COMMAND, "timeout": HOOK_TIMEOUT }
        ]
    }));
    true
}

/// Strip our SessionStart entry. Cleans up empty `SessionStart` array and
/// empty `hooks` object. Returns true if the document changed.
pub(crate) fn remove_hook(settings: &mut Value) -> bool {
    let Some(hooks) = settings.as_object_mut().and_then(|o| o.get_mut("hooks")) else {
        return false;
    };
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return false;
    };
    let Some(session_start) = hooks_obj.get_mut("SessionStart") else {
        return false;
    };
    let Some(arr) = session_start.as_array_mut() else {
        return false;
    };

    let before = arr.len();
    arr.retain(|entry| !entry_is_ours(entry));
    let changed = arr.len() != before;
    if !changed {
        return false;
    }

    if arr.is_empty() {
        hooks_obj.remove("SessionStart");
    }
    if hooks_obj.is_empty() {
        settings.as_object_mut().unwrap().remove("hooks");
    }
    true
}

fn has_our_hook(arr: &[Value]) -> bool {
    arr.iter().any(entry_is_ours)
}

fn entry_is_ours(entry: &Value) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
        return false;
    };
    hooks.iter().any(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains("skill-pool ensure"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fresh() -> Value {
        json!({})
    }

    #[test]
    fn add_to_empty() {
        let mut s = fresh();
        assert!(add_hook(&mut s));
        let arr = s["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"], "*");
        assert!(arr[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("skill-pool ensure"));
    }

    #[test]
    fn add_is_idempotent() {
        let mut s = fresh();
        assert!(add_hook(&mut s));
        assert!(!add_hook(&mut s)); // second call: no change
    }

    #[test]
    fn preserves_other_session_start_entries() {
        let mut s = json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "interactive", "hooks": [
                        { "type": "command", "command": "echo hello" }
                    ]}
                ]
            }
        });
        assert!(add_hook(&mut s));
        let arr = s["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["hooks"][0]["command"], "echo hello");
        assert!(arr[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("skill-pool ensure"));
    }

    #[test]
    fn preserves_unrelated_top_level_keys() {
        let mut s = json!({
            "model": "claude-opus-4-7",
            "permissions": { "allow": ["Read"] }
        });
        assert!(add_hook(&mut s));
        assert_eq!(s["model"], "claude-opus-4-7");
        assert_eq!(s["permissions"]["allow"][0], "Read");
    }

    #[test]
    fn remove_pulls_just_our_entry() {
        let mut s = json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "interactive", "hooks": [
                        { "type": "command", "command": "echo hello" }
                    ]},
                    { "matcher": "*", "hooks": [
                        { "type": "command", "command": "skill-pool ensure --quiet" }
                    ]}
                ]
            }
        });
        assert!(remove_hook(&mut s));
        let arr = s["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["hooks"][0]["command"], "echo hello");
    }

    #[test]
    fn remove_cleans_up_empty_objects() {
        let mut s = json!({
            "hooks": {
                "SessionStart": [
                    { "matcher": "*", "hooks": [
                        { "type": "command", "command": "skill-pool ensure --quiet" }
                    ]}
                ]
            }
        });
        assert!(remove_hook(&mut s));
        assert!(s.get("hooks").is_none(), "empty hooks block should be pruned: {s}");
    }

    #[test]
    fn remove_on_absent_is_noop() {
        let mut s = json!({"hooks": {"PreToolUse": []}});
        assert!(!remove_hook(&mut s));
    }
}
