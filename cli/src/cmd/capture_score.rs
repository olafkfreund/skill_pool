//! `skill-pool capture-score` — invoked as the Claude Code Stop hook.
//!
//! Reads the Stop hook JSON payload from stdin, parses the transcript at
//! the path it points to, runs the deterministic scorer, and persists the
//! result under `~/.skill-pool/sessions/<session_id>.json`. No LLM, no
//! network. Exits 0 even on transcript-read failures so the hook never
//! interrupts the user's flow.
//!
//! Stop hook payload shape (Claude Code 2.x, snake_case):
//!
//! ```json
//! {
//!   "session_id": "abc-123",
//!   "transcript_path": "/home/.../session.jsonl",
//!   "cwd": "/path/to/project",
//!   "hook_event_name": "Stop",
//!   "response": "...",
//!   "stop_reason": "end_turn",
//!   "tool_use_count": 0
//! }
//! ```

use std::io::Read;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::scorer;

#[derive(Debug, Deserialize)]
pub struct StopHookPayload {
    pub session_id: String,
    pub transcript_path: String,
    #[serde(default)]
    pub cwd: Option<String>,
}

pub fn run() -> Result<()> {
    // Read stdin. Fail-soft so the hook is silent on any transient error.
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        tracing::warn!(error = ?e, "capture-score: stdin read failed");
        return Ok(());
    }
    let raw = raw.trim();
    if raw.is_empty() {
        tracing::debug!("capture-score: empty stdin, nothing to score");
        return Ok(());
    }

    let payload: StopHookPayload = match serde_json::from_str(raw) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = ?e, "capture-score: bad payload, skipping");
            return Ok(());
        }
    };

    let transcript = match std::fs::read_to_string(&payload.transcript_path) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                path = %payload.transcript_path,
                error = ?e,
                "capture-score: transcript unreadable, skipping",
            );
            return Ok(());
        }
    };

    let events = scorer::parse_transcript(&transcript);

    // Load the cross-session recurrence index (Phase 5+). Failure here is
    // non-fatal — we degrade to the v1 scorer rather than blocking the hook.
    let mut index = scorer::load_recurrence_index().unwrap_or_default();
    let score =
        scorer::score_with_recurrence(&events, &payload.session_id, payload.cwd.as_deref(), &index);

    // Update the index for next time. Cheap: only if this session has a
    // distinguishable fingerprint AND it wasn't already recorded.
    if let Some(fp) = scorer::fingerprint_from_events(&events) {
        if index.touch(&fp, &payload.session_id) {
            if let Err(e) = scorer::save_recurrence_index(&index) {
                tracing::warn!(error = ?e, "capture-score: index save failed");
            }
        }
    }

    if let Err(e) = scorer::save_score(&score) {
        tracing::warn!(error = ?e, "capture-score: save failed");
    } else {
        tracing::debug!(
            session = %payload.session_id,
            score = score.score,
            "capture-score: persisted",
        );
    }
    Ok(())
}

/// `--from-file` form, for hand-running outside the hook. Reads the same
/// JSON payload from a file path.
pub fn run_from_file(path: &std::path::Path) -> Result<()> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("read payload {}", path.display()))?;
    let payload: StopHookPayload =
        serde_json::from_str(&raw).context("parse payload as Stop hook JSON")?;
    let transcript = std::fs::read_to_string(&payload.transcript_path)
        .with_context(|| format!("read transcript {}", payload.transcript_path))?;
    let events = scorer::parse_transcript(&transcript);
    let mut index = scorer::load_recurrence_index().unwrap_or_default();
    let score =
        scorer::score_with_recurrence(&events, &payload.session_id, payload.cwd.as_deref(), &index);
    if let Some(fp) = scorer::fingerprint_from_events(&events) {
        if index.touch(&fp, &payload.session_id) {
            let _ = scorer::save_recurrence_index(&index);
        }
    }
    let path = scorer::save_score(&score)?;
    println!(
        "scored {} → {} (saved to {})",
        score.session_id,
        score.score,
        path.display()
    );
    Ok(())
}
