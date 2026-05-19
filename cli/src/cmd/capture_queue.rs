//! `skill-pool capture-queue` — Phase 4 SessionEnd hook.
//!
//! Designed to fire from the Claude Code `SessionEnd` hook, once per
//! session. Reads the per-session score file written by the Stop-hook
//! scorer (`~/.skill-pool/sessions/<id>.json`) and, if the session's
//! total score is at or above the configured threshold, drops a small
//! marker file into `~/.skill-pool/queue/<id>.queued`.
//!
//! The marker is the contract between this hook and the Phase 4.6
//! capturer daemon (Slice B): the daemon consumes the queue dir,
//! processes each marker once, and deletes it on success. Until the
//! daemon lands, the existing hourly `skill-pool capture-run` keeps
//! working off the score files directly — the queue is additive, not
//! a replacement.
//!
//! Threshold sources (highest priority first):
//!   1. `--threshold` CLI flag
//!   2. `SKILL_POOL_CAPTURE_THRESHOLD` env var
//!   3. Default: 50 (lower than the scorer's `DRAFT_THRESHOLD = 100`
//!      because SessionEnd fires once at the END, so we're more
//!      tolerant: a session that scored even moderately is worth
//!      asking the LLM about).
//!
//! Like `capture-score`, every failure mode is silenced: exit 0 so the
//! Claude Code hook never interrupts the user's flow.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::scorer::{self, SessionScore};

/// Default threshold for "queue this session for capture". Intentionally
/// lower than the scorer's `DRAFT_THRESHOLD = 100` because SessionEnd
/// fires only once per session (not per turn); we err on the side of
/// surfacing more sessions to the capturer daemon, which has its own
/// generalizable-or-not LLM gate downstream.
pub const DEFAULT_THRESHOLD: u32 = 50;

/// Env var name for the threshold override.
pub const THRESHOLD_ENV: &str = "SKILL_POOL_CAPTURE_THRESHOLD";

/// JSON shape written into the marker file. Stable: the capturer daemon
/// will consume this, so the field set is intentionally minimal.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct QueueMarker {
    pub queued_at: DateTime<Utc>,
    pub session_id: String,
    pub score: u32,
    pub threshold: u32,
}

/// Resolve the queue directory next to `sessions/` under the env-resolved
/// skill-pool home. Mirrors `scorer::sessions_dir()`'s layout.
pub fn queue_dir() -> Result<PathBuf> {
    Ok(scorer::sessions_dir()?
        .parent()
        .map_or_else(|| PathBuf::from("queue"), |p| p.join("queue")))
}

/// Resolve a threshold from CLI flag → env var → default.
pub fn resolve_threshold(flag: Option<u32>) -> u32 {
    if let Some(t) = flag {
        return t;
    }
    if let Ok(raw) = std::env::var(THRESHOLD_ENV) {
        if let Ok(parsed) = raw.trim().parse::<u32>() {
            return parsed;
        }
        tracing::warn!(
            value = %raw,
            "capture-queue: {THRESHOLD_ENV} is not a valid u32; falling back to default",
        );
    }
    DEFAULT_THRESHOLD
}

/// `skill-pool capture-queue` — top-level entry point.
///
/// Session ID lookup order: explicit `--session-id` → `CLAUDE_SESSION_ID`
/// env var (what Claude Code sets when invoking hooks). If neither is
/// present we print a friendly note and exit 0 — same fail-soft policy
/// as `capture-score`.
pub fn run(session_id: Option<String>, threshold: Option<u32>) -> Result<()> {
    let resolved_session_id = match resolve_session_id(session_id) {
        Some(s) => s,
        None => {
            tracing::debug!("capture-queue: no session id available, skipping");
            println!("(no session id; nothing to queue)");
            return Ok(());
        }
    };

    let sessions = scorer::sessions_dir()?;
    let queue = queue_dir()?;
    let thr = resolve_threshold(threshold);

    match enqueue_if_above_threshold(&resolved_session_id, thr, &sessions, &queue)? {
        EnqueueOutcome::Queued { marker_path, score } => {
            println!(
                "queued session {} (score={}, threshold={}) → {}",
                short(&resolved_session_id),
                score,
                thr,
                marker_path.display(),
            );
        }
        EnqueueOutcome::BelowThreshold { score } => {
            println!(
                "session {} score {} below threshold {} — skipping",
                short(&resolved_session_id),
                score,
                thr,
            );
        }
        EnqueueOutcome::Missing => {
            println!(
                "no score record for session {}; skipping",
                short(&resolved_session_id),
            );
        }
        EnqueueOutcome::AlreadyQueued => {
            println!(
                "session {} already queued; skipping",
                short(&resolved_session_id),
            );
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Queued { marker_path: PathBuf, score: u32 },
    BelowThreshold { score: u32 },
    Missing,
    AlreadyQueued,
}

/// Pure-IO core: read the session score from `sessions_dir`, write the
/// marker (when above threshold) into `queue_dir`. Used by the public
/// `run` and the tests.
pub fn enqueue_if_above_threshold(
    session_id: &str,
    threshold: u32,
    sessions_dir: &Path,
    queue_dir: &Path,
) -> Result<EnqueueOutcome> {
    let session_path = sessions_dir.join(format!("{}.json", sanitize(session_id)));
    if !session_path.exists() {
        return Ok(EnqueueOutcome::Missing);
    }

    let raw = std::fs::read_to_string(&session_path)
        .with_context(|| format!("read {}", session_path.display()))?;
    let score: SessionScore = serde_json::from_str(&raw)
        .with_context(|| format!("parse {} as SessionScore", session_path.display()))?;

    if score.score < threshold {
        return Ok(EnqueueOutcome::BelowThreshold { score: score.score });
    }

    let marker_path = queue_dir.join(format!("{}.queued", sanitize(session_id)));
    if marker_path.exists() {
        return Ok(EnqueueOutcome::AlreadyQueued);
    }

    std::fs::create_dir_all(queue_dir)
        .with_context(|| format!("mkdir -p {}", queue_dir.display()))?;

    let marker = QueueMarker {
        queued_at: Utc::now(),
        session_id: score.session_id.clone(),
        score: score.score,
        threshold,
    };
    // Atomic write: tmp + rename, same pattern as save_score so a
    // partial flush never confuses the daemon.
    let tmp = queue_dir.join(format!(".{}.tmp", sanitize(session_id)));
    let pretty = serde_json::to_string_pretty(&marker)?;
    std::fs::write(&tmp, pretty).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &marker_path)
        .with_context(|| format!("rename {} → {}", tmp.display(), marker_path.display()))?;

    Ok(EnqueueOutcome::Queued {
        marker_path,
        score: score.score,
    })
}

fn resolve_session_id(arg: Option<String>) -> Option<String> {
    arg.filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("CLAUDE_SESSION_ID").ok())
        .filter(|s| !s.trim().is_empty())
}

/// Mirror the sanitisation used by `scorer::save_score_in` so the marker
/// path always matches the on-disk session filename.
fn sanitize(id: &str) -> String {
    id.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn short(id: &str) -> &str {
    let cut = id.char_indices().nth(8).map_or(id.len(), |(i, _)| i);
    &id[..cut]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::{save_score_in, ScoreBreakdown, SessionScore};

    fn make_score(session_id: &str, total: u32) -> SessionScore {
        SessionScore {
            session_id: session_id.to_string(),
            cwd: Some("/tmp".into()),
            score: total,
            breakdown: ScoreBreakdown::default(),
            signals: vec![],
            turn_count: 1,
            last_scored_at: Utc::now(),
            version: 2,
            capture_state: None,
        }
    }

    #[test]
    fn queues_when_score_above_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();

        let s = make_score("s-high", 200);
        save_score_in(&s, &sessions).unwrap();

        let outcome = enqueue_if_above_threshold("s-high", 50, &sessions, &queue).unwrap();
        match outcome {
            EnqueueOutcome::Queued { marker_path, score } => {
                assert_eq!(score, 200);
                assert!(marker_path.exists(), "marker file should exist");
                let raw = std::fs::read_to_string(&marker_path).unwrap();
                let m: QueueMarker = serde_json::from_str(&raw).unwrap();
                assert_eq!(m.session_id, "s-high");
                assert_eq!(m.score, 200);
                assert_eq!(m.threshold, 50);
            }
            other => panic!("expected Queued, got {other:?}"),
        }
    }

    #[test]
    fn skips_when_score_below_threshold() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();

        let s = make_score("s-low", 10);
        save_score_in(&s, &sessions).unwrap();

        let outcome = enqueue_if_above_threshold("s-low", 50, &sessions, &queue).unwrap();
        assert_eq!(outcome, EnqueueOutcome::BelowThreshold { score: 10 });
        assert!(!queue.exists() || queue.read_dir().unwrap().next().is_none());
    }

    #[test]
    fn returns_missing_when_no_score_file() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();

        let outcome = enqueue_if_above_threshold("nope", 50, &sessions, &queue).unwrap();
        assert_eq!(outcome, EnqueueOutcome::Missing);
    }

    #[test]
    fn skips_when_already_queued() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();

        let s = make_score("s-dup", 200);
        save_score_in(&s, &sessions).unwrap();

        // First enqueue succeeds.
        let first = enqueue_if_above_threshold("s-dup", 50, &sessions, &queue).unwrap();
        assert!(matches!(first, EnqueueOutcome::Queued { .. }));

        // Second is a no-op.
        let second = enqueue_if_above_threshold("s-dup", 50, &sessions, &queue).unwrap();
        assert_eq!(second, EnqueueOutcome::AlreadyQueued);
    }

    #[test]
    fn at_threshold_is_queued() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();

        // Exactly at threshold counts as ≥.
        let s = make_score("s-eq", 50);
        save_score_in(&s, &sessions).unwrap();

        let outcome = enqueue_if_above_threshold("s-eq", 50, &sessions, &queue).unwrap();
        assert!(matches!(outcome, EnqueueOutcome::Queued { .. }));
    }

    #[test]
    fn sanitises_session_id_in_filename() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();

        // Scorer sanitises path-traversal chars when saving; the queue
        // path resolution must match so the lookup actually finds it.
        let weird = "../weird/id";
        let s = make_score(weird, 200);
        save_score_in(&s, &sessions).unwrap();

        let outcome = enqueue_if_above_threshold(weird, 50, &sessions, &queue).unwrap();
        match outcome {
            EnqueueOutcome::Queued { marker_path, .. } => {
                let name = marker_path.file_name().unwrap().to_string_lossy().into_owned();
                assert!(!name.contains('/'), "no path component");
                assert!(!name.contains(".."), "no traversal");
            }
            other => panic!("expected Queued, got {other:?}"),
        }
    }

    #[test]
    fn resolve_threshold_prefers_flag_over_env() {
        // We can't safely mutate env in tests with parallel execution,
        // so test the precedence by exercising only the flag arm.
        assert_eq!(resolve_threshold(Some(7)), 7);
    }

    #[test]
    fn resolve_threshold_falls_back_to_default_when_no_flag_and_no_env() {
        // Best-effort: only assert default IF the env var is unset.
        if std::env::var(THRESHOLD_ENV).is_err() {
            assert_eq!(resolve_threshold(None), DEFAULT_THRESHOLD);
        }
    }

    #[test]
    fn corrupted_session_file_returns_error_not_panic() {
        let tmp = tempfile::tempdir().unwrap();
        let sessions = tmp.path().join("sessions");
        let queue = tmp.path().join("queue");
        std::fs::create_dir_all(&sessions).unwrap();
        std::fs::write(sessions.join("garbage.json"), "{not json").unwrap();

        let err = enqueue_if_above_threshold("garbage", 50, &sessions, &queue);
        assert!(err.is_err(), "corrupt JSON should surface as Err");
    }
}
