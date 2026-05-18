//! `skill-pool hook-install` — wires Claude Code hooks into
//! `<project>/.claude/settings.json`.
//!
//! Two hooks, both opt-in by flag:
//!   - **SessionStart** (`skill-pool ensure --quiet`) — keeps the
//!     project's skills installed; the canonical Phase 3 trigger.
//!   - **Stop** (`skill-pool capture-score`) — Phase 4.5 signal scorer;
//!     runs after every assistant turn and persists a score to
//!     `~/.skill-pool/sessions/`.
//!
//! Both complement direnv: direnv runs on shell entry; SessionStart
//! covers users who skip direnv or open Claude directly; Stop is per-turn.
//!
//! Identification: each entry we own has a `command` string containing a
//! recognised substring (`skill-pool ensure` or `skill-pool capture-score`).
//! That lets users edit our flags without losing remove/idempotency.

use anyhow::{Context, Result};
use serde_json::{json, Value};

use crate::manifest::find_project_root;

const ENSURE_COMMAND: &str = "skill-pool ensure --quiet";
const ENSURE_TIMEOUT: u64 = 30;
const SCORE_COMMAND: &str = "skill-pool capture-score";
const SCORE_TIMEOUT: u64 = 10;

pub fn run(remove: bool, print_only: bool, with_scorer: bool) -> Result<()> {
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

    let mut changed = false;
    let mut messages: Vec<String> = Vec::new();

    if remove {
        if remove_event(&mut settings, "SessionStart", "skill-pool ensure") {
            changed = true;
            messages.push("removed SessionStart hook".into());
        }
        if remove_event(&mut settings, "Stop", "skill-pool capture-score") {
            changed = true;
            messages.push("removed Stop hook".into());
        }
    } else {
        if add_event(
            &mut settings,
            "SessionStart",
            "skill-pool ensure",
            ENSURE_COMMAND,
            ENSURE_TIMEOUT,
        ) {
            changed = true;
            messages.push(format!("installed SessionStart → `{ENSURE_COMMAND}`"));
        }
        if with_scorer
            && add_event(
                &mut settings,
                "Stop",
                "skill-pool capture-score",
                SCORE_COMMAND,
                SCORE_TIMEOUT,
            )
        {
            changed = true;
            messages.push(format!("installed Stop → `{SCORE_COMMAND}`"));
        }
    }

    if print_only {
        println!("{}", serde_json::to_string_pretty(&settings)?);
        return Ok(());
    }

    if !changed {
        if remove {
            println!("(no skill-pool hooks found in {})", settings_path.display());
        } else if with_scorer {
            println!(
                "(skill-pool SessionStart + Stop hooks already present in {})",
                settings_path.display()
            );
        } else {
            println!(
                "(skill-pool SessionStart hook already present in {})",
                settings_path.display()
            );
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

    for m in &messages {
        println!("{m}");
    }
    println!("→ wrote {}", settings_path.display());
    Ok(())
}

/// Merge `(event, command)` into `settings.hooks.<event>`. Returns true if
/// the document changed. Idempotent: if an entry whose command contains
/// `marker` already exists under that event, no-op.
pub(crate) fn add_event(
    settings: &mut Value,
    event: &str,
    marker: &str,
    command: &str,
    timeout: u64,
) -> bool {
    let hooks = settings
        .as_object_mut()
        .and_then(|o| {
            o.entry("hooks".to_string()).or_insert(json!({}));
            o.get_mut("hooks")
        })
        .and_then(|h| h.as_object_mut())
        .expect("settings root is an object after construction");

    let arr_val = hooks.entry(event.to_string()).or_insert(json!([]));
    let arr = match arr_val.as_array_mut() {
        Some(a) => a,
        None => return false,
    };

    if arr.iter().any(|e| entry_matches(e, marker)) {
        return false;
    }

    arr.push(json!({
        "matcher": "*",
        "hooks": [
            { "type": "command", "command": command, "timeout": timeout }
        ]
    }));
    true
}

/// Strip every entry under `event` whose command contains `marker`.
/// Cleans up empty `<event>` array and empty `hooks` object so the file
/// stays tidy. Returns true if the document changed.
pub(crate) fn remove_event(settings: &mut Value, event: &str, marker: &str) -> bool {
    let Some(hooks) = settings.as_object_mut().and_then(|o| o.get_mut("hooks")) else {
        return false;
    };
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return false;
    };
    let Some(event_val) = hooks_obj.get_mut(event) else {
        return false;
    };
    let Some(arr) = event_val.as_array_mut() else {
        return false;
    };

    let before = arr.len();
    arr.retain(|entry| !entry_matches(entry, marker));
    let changed = arr.len() != before;
    if !changed {
        return false;
    }

    if arr.is_empty() {
        hooks_obj.remove(event);
    }
    if hooks_obj.is_empty() {
        settings.as_object_mut().unwrap().remove("hooks");
    }
    true
}

fn entry_matches(entry: &Value, marker: &str) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
        return false;
    };
    hooks.iter().any(|h| {
        h.get("command")
            .and_then(|c| c.as_str())
            .is_some_and(|c| c.contains(marker))
    })
}

// Backwards-compatible aliases. Kept as thin wrappers so old tests that
// referenced the SessionStart-only helpers keep compiling, and so external
// callers (none in-tree today) don't break.
#[cfg(test)]
pub(crate) fn add_hook(settings: &mut Value) -> bool {
    add_event(
        settings,
        "SessionStart",
        "skill-pool ensure",
        ENSURE_COMMAND,
        ENSURE_TIMEOUT,
    )
}

#[cfg(test)]
pub(crate) fn remove_hook(settings: &mut Value) -> bool {
    remove_event(settings, "SessionStart", "skill-pool ensure")
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
        assert!(!add_hook(&mut s));
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

    #[test]
    fn add_stop_event_for_scorer() {
        let mut s = fresh();
        assert!(add_event(
            &mut s,
            "Stop",
            "skill-pool capture-score",
            SCORE_COMMAND,
            SCORE_TIMEOUT
        ));
        let arr = s["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(arr[0]["hooks"][0]["command"], SCORE_COMMAND);
    }

    #[test]
    fn session_start_and_stop_coexist_and_remove_independently() {
        let mut s = fresh();
        // Install both.
        assert!(add_event(
            &mut s,
            "SessionStart",
            "skill-pool ensure",
            ENSURE_COMMAND,
            ENSURE_TIMEOUT
        ));
        assert!(add_event(
            &mut s,
            "Stop",
            "skill-pool capture-score",
            SCORE_COMMAND,
            SCORE_TIMEOUT
        ));
        assert!(s["hooks"]["SessionStart"].is_array());
        assert!(s["hooks"]["Stop"].is_array());

        // Remove only Stop — SessionStart untouched.
        assert!(remove_event(&mut s, "Stop", "skill-pool capture-score"));
        assert!(s["hooks"].get("Stop").is_none(), "Stop should be pruned");
        assert!(s["hooks"]["SessionStart"].is_array(), "SessionStart kept");
    }
}
