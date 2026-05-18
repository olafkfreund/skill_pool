//! Phase 4.5 signal scorer.
//!
//! Cheap (<50ms), deterministic, **no LLM**. Reads a Claude Code transcript
//! JSONL and scores it against four high-precision signals from the master
//! plan:
//!
//!  1. **Explicit markers** in user messages (`"remember this"`, `"TIL"`,
//!     `/capture-skill`) → weight 1000 (auto-fire).
//!  2. **Failing → passing test recovery** — same test command failed ≥2
//!     times then passed → weight 50.
//!  3. **Edit retries on a single file** — Edit/Write failures > 3 on the
//!     same path → weight 30 per excess file.
//!  4. **Long session on one task** — more than 20 assistant turns → weight 5.
//!
//! The scorer is invoked by the Stop hook every assistant turn. Output is
//! persisted to `~/.skill-pool/sessions/<session_id>.json` so a later
//! capturer daemon (Phase 4.6) can decide whether to draft.
//!
//! Cross-session recurrence and "novel command vs shell history" are
//! deferred — both require historical state outside this CLI, and the v1
//! signals already correlate with them well enough to be a useful prefilter.

use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Score considered "draft-worthy" — capturer (Phase 4.6) will fire above
/// this. Lives here so the scorer is self-contained.
pub const DRAFT_THRESHOLD: u32 = 100;

const W_EXPLICIT: u32 = 1000;
const W_TEST_RECOVERY: u32 = 50;
const W_EDIT_RETRY_PER_FILE: u32 = 30;
const W_LONG_SESSION: u32 = 5;

const LONG_SESSION_TURNS: usize = 20;
const EDIT_RETRY_MIN_FAILURES: usize = 3;
const TEST_RECOVERY_MIN_FAILURES: usize = 2;

/// Explicit "save this" markers. Lowercased substring match; cheap. Order
/// reflects the master plan's wording.
const EXPLICIT_MARKERS: &[&str] = &[
    "remember this",
    "save this",
    "let's capture",
    "let us capture",
    "/capture-skill",
    "til:",
    " til ",
];

/// The output schema; persisted per session.
///
/// `capture_state` is optional so v1 records (which lack the field) still
/// deserialize. New writes always carry it; the `version` field is the
/// canonical signal that this record was written by a Phase 4.6+ scorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionScore {
    pub session_id: String,
    pub cwd: Option<String>,
    pub score: u32,
    pub breakdown: ScoreBreakdown,
    pub signals: Vec<Signal>,
    pub turn_count: usize,
    pub last_scored_at: DateTime<Utc>,
    /// Schema version — bumped if the on-disk shape changes.
    pub version: u32,
    /// Records the capturer's outcome for this session, if it has run.
    /// Used for idempotency: a session with `capture_state.is_some()` is
    /// never re-processed by the capturer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_state: Option<CaptureState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureState {
    /// What the capturer decided on this session.
    pub stage: CaptureStage,
    /// When the capturer finished processing.
    pub completed_at: DateTime<Utc>,
    /// Set when `stage == Drafted`. The draft UUID returned by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draft_id: Option<String>,
    /// Slug of the produced draft, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slug: Option<String>,
    /// Why we ended in this stage (e.g. Stage 1 said "too project-specific").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureStage {
    /// Stage 1 returned `generalizable: false`. No draft created.
    Stage1Rejected,
    /// Stage 1 output failed to parse as JSON twice in a row. Future
    /// improvements may re-attempt with a stricter prompt.
    Stage1ParseFailure,
    /// Stage 2 produced a SKILL.md but client-side validation rejected it
    /// (secret scan, frontmatter shape, etc.). No draft created.
    Stage2Rejected,
    /// Stage 2 produced a draft and the server accepted it.
    Drafted,
    /// Server rejected the POST (e.g. dedupe collision, network error
    /// retries exhausted). The score record stays so a later, fixed run
    /// can retry — but the capturer treats it as processed for this pass.
    ServerRejected,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ScoreBreakdown {
    pub explicit_markers: u32,
    pub test_recovery: u32,
    pub edit_retries: u32,
    pub long_session: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Signal {
    pub kind: SignalKind,
    pub weight: u32,
    /// One-line summary that survives in the persisted JSON for human
    /// inspection. Trimmed; never exceeds 240 chars.
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    ExplicitMarker,
    TestRecovery,
    EditRetry,
    LongSession,
}

// ---------------------------------------------------------------------------
// Event model — we deliberately project the rich Claude transcript into a
// small enum so the scoring rules can be tested without touching JSONL.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    UserText(String),
    AssistantText,
    ToolUse {
        name: String,
        /// Best-effort "what this tool is acting on". Edit/Write use the
        /// `file_path` input; Bash uses the `command` string trimmed.
        target: Option<String>,
    },
    ToolResult {
        is_error: bool,
        /// Best-effort body for evidence strings; truncated to keep memory
        /// usage bounded.
        body: String,
    },
}

/// Parse a transcript JSONL string into the projected `Event` stream.
/// Robust to schema drift: unknown / malformed lines are skipped rather
/// than failing the whole scorer.
pub fn parse_transcript(raw: &str) -> Vec<Event> {
    let mut events = Vec::with_capacity(raw.lines().count());
    for line in raw.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(ty) = v.get("type").and_then(|t| t.as_str()) else {
            continue;
        };
        let content = v.get("message").and_then(|m| m.get("content"));
        match ty {
            "user" => match content {
                Some(Value::String(s)) => {
                    if !s.starts_with('<') {
                        events.push(Event::UserText(s.clone()));
                    }
                }
                Some(Value::Array(parts)) => {
                    for part in parts {
                        if part.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                            let is_error = part
                                .get("is_error")
                                .and_then(|b| b.as_bool())
                                .unwrap_or(false);
                            let body = match part.get("content") {
                                Some(Value::String(s)) => s.clone(),
                                Some(Value::Array(arr)) => arr
                                    .iter()
                                    .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                _ => String::new(),
                            };
                            // Heuristic: a tool result that wraps its body in
                            // <tool_use_error> is an error even if the
                            // is_error flag was omitted.
                            let is_error = is_error || body.contains("<tool_use_error>");
                            events.push(Event::ToolResult {
                                is_error,
                                body: truncate(&body, 4096),
                            });
                        }
                    }
                }
                _ => {}
            },
            "assistant" => {
                let Some(Value::Array(parts)) = content else {
                    continue;
                };
                let mut produced_tool_use = false;
                for part in parts {
                    let pt = part.get("type").and_then(|t| t.as_str());
                    match pt {
                        Some("tool_use") => {
                            produced_tool_use = true;
                            let name = part
                                .get("name")
                                .and_then(|n| n.as_str())
                                .unwrap_or("")
                                .to_string();
                            let target = extract_target(&name, part.get("input"));
                            events.push(Event::ToolUse { name, target });
                        }
                        Some("text") => {
                            // Per-text-part assistant events are noise for
                            // turn counting; we record one AssistantText per
                            // line, but only if no tool_use accompanies it
                            // (else the tool_use is the better signal).
                        }
                        _ => {}
                    }
                }
                if !produced_tool_use {
                    events.push(Event::AssistantText);
                }
            }
            _ => {}
        }
    }
    events
}

fn extract_target(tool_name: &str, input: Option<&Value>) -> Option<String> {
    let input = input?;
    let key = match tool_name {
        "Edit" | "Write" | "Read" | "NotebookEdit" => "file_path",
        "Bash" => "command",
        _ => return None,
    };
    let raw = input.get(key)?.as_str()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate(trimmed, 240))
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut end = max.min(s.len());
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}…", &s[..end])
    }
}

// ---------------------------------------------------------------------------
// Scoring rules — pure functions over `&[Event]`.
// ---------------------------------------------------------------------------

pub fn score(events: &[Event], session_id: &str, cwd: Option<&str>) -> SessionScore {
    let mut signals: Vec<Signal> = Vec::new();
    let mut breakdown = ScoreBreakdown::default();

    if let Some(sig) = rule_explicit(events) {
        breakdown.explicit_markers = sig.weight;
        signals.push(sig);
    }
    let test_sigs = rule_test_recovery(events);
    for sig in &test_sigs {
        breakdown.test_recovery = breakdown.test_recovery.saturating_add(sig.weight);
    }
    signals.extend(test_sigs);

    let retry_sigs = rule_edit_retry(events);
    for sig in &retry_sigs {
        breakdown.edit_retries = breakdown.edit_retries.saturating_add(sig.weight);
    }
    signals.extend(retry_sigs);

    let turn_count = count_assistant_turns(events);
    if let Some(sig) = rule_long_session(turn_count) {
        breakdown.long_session = sig.weight;
        signals.push(sig);
    }

    let total = breakdown
        .explicit_markers
        .saturating_add(breakdown.test_recovery)
        .saturating_add(breakdown.edit_retries)
        .saturating_add(breakdown.long_session);

    SessionScore {
        session_id: session_id.to_string(),
        cwd: cwd.map(String::from),
        score: total,
        breakdown,
        signals,
        turn_count,
        last_scored_at: Utc::now(),
        version: 2,
        capture_state: None,
    }
}

fn rule_explicit(events: &[Event]) -> Option<Signal> {
    for ev in events {
        let Event::UserText(text) = ev else {
            continue;
        };
        let lc = text.to_lowercase();
        for marker in EXPLICIT_MARKERS {
            if lc.contains(marker) {
                return Some(Signal {
                    kind: SignalKind::ExplicitMarker,
                    weight: W_EXPLICIT,
                    evidence: truncate(&format!("user said `{}`", marker.trim()), 240),
                });
            }
        }
    }
    None
}

fn rule_test_recovery(events: &[Event]) -> Vec<Signal> {
    // State machine: for each Bash command that looks like a test, count
    // consecutive failures, then award the signal when we see a passing
    // run with >= TEST_RECOVERY_MIN_FAILURES preceding failures.
    let mut signals = Vec::new();
    let mut current_cmd: Option<String> = None;
    let mut current_fail_streak: usize = 0;

    let mut iter = events.iter().peekable();
    while let Some(ev) = iter.next() {
        let Event::ToolUse { name, target } = ev else {
            continue;
        };
        if name != "Bash" {
            continue;
        }
        let Some(cmd) = target else { continue };
        if !looks_like_test(cmd) {
            continue;
        }
        // Expect the very next event to be the matching ToolResult.
        let is_error = match iter.peek() {
            Some(Event::ToolResult { is_error, .. }) => *is_error,
            _ => continue,
        };
        // Consume the result so we don't double-count.
        iter.next();

        // Same logical command? Compare on the normalized command string.
        let same = current_cmd.as_deref() == Some(cmd.as_str());
        if !same {
            current_cmd = Some(cmd.clone());
            current_fail_streak = 0;
        }
        if is_error {
            current_fail_streak = current_fail_streak.saturating_add(1);
        } else if current_fail_streak >= TEST_RECOVERY_MIN_FAILURES {
            signals.push(Signal {
                kind: SignalKind::TestRecovery,
                weight: W_TEST_RECOVERY,
                evidence: truncate(
                    &format!(
                        "`{}` failed {}× then passed",
                        cmd, current_fail_streak
                    ),
                    240,
                ),
            });
            // Reset so a later second recovery on the same test counts again.
            current_fail_streak = 0;
        } else {
            current_fail_streak = 0;
        }
    }
    signals
}

fn looks_like_test(cmd: &str) -> bool {
    let lc = cmd.to_lowercase();
    // Match the first non-trivial token chain. Avoid `find . -name ...`-style false positives.
    let patterns: [&str; 8] = [
        "cargo test",
        "cargo nextest",
        "pytest",
        "npm test",
        "npm run test",
        "yarn test",
        "go test",
        "jest",
    ];
    patterns.iter().any(|p| lc.contains(p))
}

fn rule_edit_retry(events: &[Event]) -> Vec<Signal> {
    use std::collections::HashMap;
    // For each Edit/Write call, the *next* event is its ToolResult. We
    // tally failures per `target` (the file_path).
    let mut per_file: HashMap<String, usize> = HashMap::new();
    let mut iter = events.iter().peekable();
    while let Some(ev) = iter.next() {
        let Event::ToolUse { name, target } = ev else {
            continue;
        };
        if name != "Edit" && name != "Write" {
            continue;
        }
        let Some(path) = target else { continue };
        let is_error = match iter.peek() {
            Some(Event::ToolResult { is_error, .. }) => *is_error,
            _ => continue,
        };
        iter.next();
        if is_error {
            *per_file.entry(path.clone()).or_default() += 1;
        }
    }
    per_file
        .into_iter()
        .filter(|(_, n)| *n > EDIT_RETRY_MIN_FAILURES)
        .map(|(path, n)| Signal {
            kind: SignalKind::EditRetry,
            weight: W_EDIT_RETRY_PER_FILE,
            evidence: truncate(&format!("{n} failed edits on {path}"), 240),
        })
        .collect()
}

fn rule_long_session(turn_count: usize) -> Option<Signal> {
    if turn_count > LONG_SESSION_TURNS {
        Some(Signal {
            kind: SignalKind::LongSession,
            weight: W_LONG_SESSION,
            evidence: format!("{turn_count} assistant turns in this session"),
        })
    } else {
        None
    }
}

fn count_assistant_turns(events: &[Event]) -> usize {
    // Count each contiguous run of assistant emissions (text or any number
    // of tool_uses) as one logical turn. A turn boundary is any UserText
    // or ToolResult that comes BEFORE the next assistant event.
    let mut turns = 0;
    let mut in_turn = false;
    for ev in events {
        match ev {
            Event::AssistantText | Event::ToolUse { .. } => {
                if !in_turn {
                    turns += 1;
                    in_turn = true;
                }
            }
            Event::UserText(_) | Event::ToolResult { .. } => {
                in_turn = false;
            }
        }
    }
    turns
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// Resolve the default sessions directory from the environment.
/// `SKILL_POOL_HOME` wins; otherwise `$XDG_DATA_HOME/skill-pool` or
/// `~/.skill-pool`. The `sessions/` leaf is always appended.
pub fn sessions_dir() -> Result<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("SKILL_POOL_HOME") {
        return Ok(std::path::PathBuf::from(dir).join("sessions"));
    }
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(s) if !s.is_empty() => std::path::PathBuf::from(s).join("skill-pool"),
        _ => {
            let home = std::env::var("HOME").context("HOME not set")?;
            std::path::PathBuf::from(home).join(".skill-pool")
        }
    };
    Ok(base.join("sessions"))
}

/// Atomic write: tmp file in same dir + rename, so a partial flush never
/// leaves a half-written score behind. Uses the env-resolved sessions dir.
pub fn save_score(score: &SessionScore) -> Result<std::path::PathBuf> {
    save_score_in(score, &sessions_dir()?)
}

/// Like `save_score` but writes under an explicit directory. Used by tests
/// (and conceivably by callers who want to keep multiple session stores).
pub fn save_score_in(score: &SessionScore, dir: &Path) -> Result<std::path::PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("mkdir -p {}", dir.display()))?;
    let final_path = dir.join(format!("{}.json", sanitize(&score.session_id)));
    let tmp_path = dir.join(format!(".{}.tmp", sanitize(&score.session_id)));
    let pretty = serde_json::to_string_pretty(score)?;
    std::fs::write(&tmp_path, pretty)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(final_path)
}

fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect()
}

/// Read every session score in the env-resolved sessions dir, sorted by
/// score descending. Single corrupt files are logged and skipped.
pub fn load_all_scores() -> Result<Vec<SessionScore>> {
    load_all_scores_in(&sessions_dir()?)
}

/// True for a session that meets the draft threshold AND has not yet been
/// processed by the capturer. The orchestrator uses this to pick work each
/// pass.
pub fn needs_capturing(s: &SessionScore) -> bool {
    s.score >= DRAFT_THRESHOLD && s.capture_state.is_none()
}

pub fn load_all_scores_in(dir: &Path) -> Result<Vec<SessionScore>> {
    if !dir.exists() {
        return Ok(vec![]);
    }
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        match read_score(&path) {
            Ok(s) => out.push(s),
            Err(e) => tracing::warn!(file = %path.display(), error = ?e, "skip unreadable score"),
        }
    }
    out.sort_by(|a, b| b.score.cmp(&a.score).then(b.last_scored_at.cmp(&a.last_scored_at)));
    Ok(out)
}

fn read_score(path: &Path) -> Result<SessionScore> {
    let raw = std::fs::read_to_string(path)?;
    Ok(serde_json::from_str(&raw)?)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn user(s: &str) -> Event {
        Event::UserText(s.into())
    }
    fn bash_cmd(cmd: &str) -> Event {
        Event::ToolUse {
            name: "Bash".into(),
            target: Some(cmd.into()),
        }
    }
    fn edit(path: &str) -> Event {
        Event::ToolUse {
            name: "Edit".into(),
            target: Some(path.into()),
        }
    }
    fn ok_result() -> Event {
        Event::ToolResult {
            is_error: false,
            body: String::new(),
        }
    }
    fn err_result() -> Event {
        Event::ToolResult {
            is_error: true,
            body: String::new(),
        }
    }
    fn ass() -> Event {
        Event::AssistantText
    }

    #[test]
    fn explicit_marker_fires_on_remember_this() {
        let s = score(
            &[user("please remember this for next time")],
            "s1",
            Some("/tmp"),
        );
        assert!(s.score >= W_EXPLICIT, "{s:?}");
        assert!(s
            .signals
            .iter()
            .any(|x| x.kind == SignalKind::ExplicitMarker));
    }

    #[test]
    fn explicit_marker_fires_on_capture_skill_slash() {
        let s = score(&[user("/capture-skill")], "s2", None);
        assert_eq!(s.breakdown.explicit_markers, W_EXPLICIT);
    }

    #[test]
    fn explicit_marker_is_case_insensitive() {
        let s = score(&[user("TIL: tar -C does X")], "s2b", None);
        assert_eq!(s.breakdown.explicit_markers, W_EXPLICIT);
    }

    #[test]
    fn no_signals_on_quiet_session() {
        let s = score(
            &[user("hi"), ass(), user("thanks"), ass()],
            "quiet",
            None,
        );
        assert_eq!(s.score, 0);
        assert!(s.signals.is_empty());
    }

    #[test]
    fn test_recovery_fires_on_two_fails_then_pass() {
        let s = score(
            &[
                user("run tests"),
                bash_cmd("cargo test"),
                err_result(),
                bash_cmd("cargo test"),
                err_result(),
                bash_cmd("cargo test"),
                ok_result(),
            ],
            "tr1",
            None,
        );
        assert_eq!(s.breakdown.test_recovery, W_TEST_RECOVERY, "{s:?}");
    }

    #[test]
    fn test_recovery_skips_when_only_one_fail() {
        let s = score(
            &[
                bash_cmd("cargo test"),
                err_result(),
                bash_cmd("cargo test"),
                ok_result(),
            ],
            "tr2",
            None,
        );
        assert_eq!(s.breakdown.test_recovery, 0);
    }

    #[test]
    fn test_recovery_skips_non_test_bash() {
        let s = score(
            &[
                bash_cmd("ls -la"),
                err_result(),
                bash_cmd("ls -la"),
                err_result(),
                bash_cmd("ls -la"),
                ok_result(),
            ],
            "tr3",
            None,
        );
        assert_eq!(s.breakdown.test_recovery, 0);
    }

    #[test]
    fn edit_retry_fires_on_four_fails_same_file() {
        let mut ev = Vec::new();
        for _ in 0..4 {
            ev.push(edit("/x/y.rs"));
            ev.push(err_result());
        }
        let s = score(&ev, "e1", None);
        assert_eq!(s.breakdown.edit_retries, W_EDIT_RETRY_PER_FILE, "{s:?}");
    }

    #[test]
    fn edit_retry_skips_three_fails() {
        let mut ev = Vec::new();
        for _ in 0..3 {
            ev.push(edit("/x/y.rs"));
            ev.push(err_result());
        }
        let s = score(&ev, "e2", None);
        assert_eq!(s.breakdown.edit_retries, 0);
    }

    #[test]
    fn edit_retry_separate_files_dont_aggregate() {
        let ev = vec![
            edit("/a.rs"),
            err_result(),
            edit("/a.rs"),
            err_result(),
            edit("/b.rs"),
            err_result(),
            edit("/b.rs"),
            err_result(),
        ];
        let s = score(&ev, "e3", None);
        assert_eq!(s.breakdown.edit_retries, 0);
    }

    #[test]
    fn long_session_fires_over_20_turns() {
        let mut ev = Vec::new();
        for _ in 0..21 {
            ev.push(user("hi"));
            ev.push(ass());
        }
        let s = score(&ev, "long", None);
        assert_eq!(s.breakdown.long_session, W_LONG_SESSION);
        assert_eq!(s.turn_count, 21);
    }

    #[test]
    fn long_session_doesnt_fire_at_20_turns() {
        let mut ev = Vec::new();
        for _ in 0..20 {
            ev.push(user("hi"));
            ev.push(ass());
        }
        let s = score(&ev, "med", None);
        assert_eq!(s.breakdown.long_session, 0);
    }

    #[test]
    fn turn_count_counts_assistant_runs_separated_by_tool_results() {
        // Claude Code emits a fresh assistant message after each tool
        // result. The scorer counts each such run as one logical turn,
        // matching how a reviewer reads the transcript.
        let ev = vec![
            user("do x"),
            ass(),
            bash_cmd("ls"),
            ok_result(),
            ass(),
            user("now y"),
            edit("/a.rs"),
            ok_result(),
            ass(),
        ];
        let s = score(&ev, "t1", None);
        // assistant_text (1) → bash_use → tool_result → assistant_text (2)
        // → user → edit_use (3) → tool_result → assistant_text (4)
        assert_eq!(s.turn_count, 4, "{s:?}");
    }

    #[test]
    fn parse_transcript_handles_user_string_and_tool_result_array() {
        let jsonl = r#"
{"type":"user","message":{"content":"hello"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"hi"}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"x","content":"<tool_use_error>nope</tool_use_error>"}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Bash","input":{"command":"cargo test"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"y","content":"ok","is_error":false}]}}
"#;
        let events = parse_transcript(jsonl);
        assert_eq!(events.len(), 5, "{events:?}");
        assert!(matches!(&events[0], Event::UserText(s) if s == "hello"));
        assert!(matches!(&events[1], Event::AssistantText));
        assert!(
            matches!(&events[2], Event::ToolResult { is_error: true, .. }),
            "tool_use_error body should imply is_error=true"
        );
        assert!(
            matches!(&events[3], Event::ToolUse { name, target: Some(t) } if name=="Bash" && t == "cargo test")
        );
        assert!(matches!(&events[4], Event::ToolResult { is_error: false, .. }));
    }

    #[test]
    fn parse_transcript_skips_garbage_lines() {
        let jsonl = "not json\n{\"type\":\"user\",\"message\":{\"content\":\"hi\"}}\n";
        let events = parse_transcript(jsonl);
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn save_and_load_round_trip() {
        // Use an explicit dir so parallel tests can't collide via env vars.
        let tmp = tempfile::tempdir().unwrap();
        let s = score(&[user("remember this")], "abc-123", Some("/x"));
        let p = save_score_in(&s, tmp.path()).expect("save");
        assert!(p.exists());

        let loaded = load_all_scores_in(tmp.path()).expect("load");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].session_id, "abc-123");
        assert_eq!(loaded[0].score, W_EXPLICIT);
    }

    #[test]
    fn save_sanitises_session_id() {
        let tmp = tempfile::tempdir().unwrap();
        let s = score(&[], "../weird/id", None);
        let p = save_score_in(&s, tmp.path()).unwrap();
        // Sanitised: no path components, no '.', no '/'
        let name = p.file_name().unwrap().to_string_lossy().into_owned();
        assert!(!name.contains('/'));
        assert!(!name.contains(".."));
    }

    #[test]
    fn v1_records_still_deserialize() {
        // Pre-Phase-4.6 records lack `capture_state`. They must still load.
        let raw = r#"{
            "session_id": "old",
            "cwd": "/x",
            "score": 1050,
            "breakdown": {
                "explicit_markers": 1000, "test_recovery": 50,
                "edit_retries": 0, "long_session": 0
            },
            "signals": [],
            "turn_count": 5,
            "last_scored_at": "2026-05-01T00:00:00Z",
            "version": 1
        }"#;
        let s: SessionScore = serde_json::from_str(raw).expect("v1 should load");
        assert_eq!(s.version, 1);
        assert!(s.capture_state.is_none());
        assert!(needs_capturing(&s), "draft-worthy and unprocessed");
    }

    #[test]
    fn needs_capturing_skips_below_threshold() {
        let s = score(&[], "low", None);
        assert_eq!(s.score, 0);
        assert!(!needs_capturing(&s));
    }

    #[test]
    fn needs_capturing_skips_already_processed() {
        let mut s = score(&[user("remember this")], "p1", None);
        assert!(needs_capturing(&s));
        s.capture_state = Some(CaptureState {
            stage: CaptureStage::Drafted,
            completed_at: Utc::now(),
            draft_id: Some("xx".into()),
            slug: Some("yy".into()),
            reason: None,
        });
        assert!(!needs_capturing(&s));
    }

    #[test]
    fn load_skips_non_json_files() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("README.md"), "ignore me").unwrap();
        let s = score(&[user("remember this")], "valid", None);
        save_score_in(&s, tmp.path()).unwrap();
        let loaded = load_all_scores_in(tmp.path()).expect("load");
        assert_eq!(loaded.len(), 1);
    }
}
