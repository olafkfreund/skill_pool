//! `skill-pool-capturer` — long-lived queue-consumer daemon.
//!
//! A second driver for the same Phase 4.6 capture pipeline that
//! `skill-pool capture-run` exposes as a single-shot. Users pick one
//! mode per host:
//!
//!  - **single-shot** (default): hourly systemd timer runs
//!    `skill-pool capture-run --limit 5`. Lower idle cost; up to ~1h
//!    latency between session end and the desktop toast.
//!  - **daemon** (this binary): a persistent loop polls every 30s and
//!    drafts each candidate within one cycle. No hourly LLM burst.
//!
//! Both share the same orchestrator code in `cmd::capture_run` — the
//! daemon is just a different driver, never a separate pipeline.
//!
//! The poll loop scans two sources for fresh work:
//!  1. `~/.skill-pool/queue/*.queued` marker files (Slice A's
//!     SessionEnd hook writes these), and
//!  2. `~/.skill-pool/sessions/*.json` for any persisted score whose
//!     `capture_state` is unset and whose `score >= DRAFT_THRESHOLD`.
//!
//! (2) is the safety net: the daemon works even when the hook isn't
//! installed yet.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::time::{interval, MissedTickBehavior};
use tracing_subscriber::EnvFilter;

use skill_pool_cli::anthropic::AnthropicClient;
use skill_pool_cli::capturer::{AnthropicStages, DEFAULT_STAGE1_MODEL, DEFAULT_STAGE2_MODEL};
use skill_pool_cli::client::Client;
use skill_pool_cli::cmd::capture_run::{find_transcript_for_session, process_one};
use skill_pool_cli::config::Config;
use skill_pool_cli::scorer::{self, SessionScore};

/// Default poll cadence. Short enough that "I just finished a hard
/// session" → "toast on my desktop" feels reactive, long enough that we
/// don't hammer the local filesystem for nothing.
const DEFAULT_POLL_SECS: u64 = 30;

#[derive(Parser, Debug)]
#[command(
    name = "skill-pool-capturer",
    version,
    about = "Long-lived skill-pool capturer daemon (Phase 4.6)",
    long_about = "Queue-consumer daemon for the Phase 4.6 capturer pipeline. \
Watches ~/.skill-pool/queue and ~/.skill-pool/sessions for draft-worthy \
sessions and runs the two-stage LLM pipeline against each one. Reuses the \
same orchestrator as `skill-pool capture-run` — exists alongside the timer-\
driven single-shot for users who want lower-latency drafts."
)]
struct Cli {
    /// Path to config file (defaults to platform-standard config dir).
    #[arg(long, env = "SKILL_POOL_CONFIG")]
    config: Option<PathBuf>,

    /// Override the registry URL for this invocation.
    #[arg(long, env = "SKILL_POOL_REGISTRY")]
    registry: Option<String>,

    /// Maximum sessions to process per poll iteration. The default of 1
    /// keeps memory bounded and limits Anthropic burst — one draft per
    /// tick rather than draining the entire queue in one go.
    #[arg(long, default_value_t = 1)]
    batch: usize,

    /// Poll interval in seconds. Also settable via
    /// `SKILL_POOL_CAPTURER_POLL_SECS`.
    #[arg(long, env = "SKILL_POOL_CAPTURER_POLL_SECS", default_value_t = DEFAULT_POLL_SECS)]
    poll_secs: u64,

    /// Override Stage 1 (extractor) model.
    #[arg(long)]
    stage1_model: Option<String>,

    /// Override Stage 2 (drafter) model.
    #[arg(long)]
    stage2_model: Option<String>,

    /// Skip the secret-scan quality gate. Findings are logged as warnings
    /// but the pipeline proceeds. The server runs its own scan too.
    #[arg(long)]
    allow_secret: bool,

    /// Suppress the per-draft desktop notification. Also settable via
    /// `SKILL_POOL_CAPTURE_NO_NOTIFY=1`. Even when this is false, the
    /// notification is gated on `DBUS_SESSION_BUS_ADDRESS` being set —
    /// systemd `Type=simple` units without a session bus see no toast,
    /// not a crash.
    #[arg(long, env = "SKILL_POOL_CAPTURE_NO_NOTIFY")]
    no_notify: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,skill_pool_cli=info")),
        )
        .init();

    let cli = Cli::parse();
    let cfg = Config::load(cli.config.as_deref(), cli.registry.as_deref())?;
    let _ = cfg.require_registry().with_context(|| {
        "no registry configured — run `skill-pool login --registry URL --tenant SLUG` first"
    })?;

    let stage1 = cli
        .stage1_model
        .as_deref()
        .unwrap_or(DEFAULT_STAGE1_MODEL)
        .to_string();
    let stage2 = cli
        .stage2_model
        .as_deref()
        .unwrap_or(DEFAULT_STAGE2_MODEL)
        .to_string();

    let poll = Duration::from_secs(cli.poll_secs.max(1));
    tracing::info!(
        poll_secs = poll.as_secs(),
        batch = cli.batch,
        stage1 = %stage1,
        stage2 = %stage2,
        "skill-pool capturer daemon starting"
    );

    // Build the long-lived clients once; the inner loop reuses them.
    let llm = AnthropicClient::from_env()?;
    let reg_cfg = cfg.require_registry()?;
    let registry = Client::new(reg_cfg)?;
    let stages = AnthropicStages {
        client: &llm,
        stage1_model: &stage1,
        stage2_model: &stage2,
    };
    let notify_enabled = !cli.no_notify;
    let web_url = cfg.web_url.clone();

    let mut tick = interval(poll);
    // Skip missed ticks so a long stage-2 call doesn't queue up a burst
    // of catch-up iterations once it returns.
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);

    // Initial tick fires immediately — we want one pass right after
    // boot so a queued session from before service start doesn't sit
    // for 30 seconds.
    loop {
        tokio::select! {
            biased;

            res = tokio::signal::ctrl_c() => {
                if let Err(e) = res {
                    tracing::warn!(error = ?e, "ctrl_c handler failed; treating as shutdown");
                }
                tracing::info!("shutdown signal received; draining");
                break;
            }
            _ = tick.tick() => {
                if let Err(e) = poll_once(
                    &stages,
                    &registry,
                    cli.batch,
                    cli.allow_secret,
                    notify_enabled,
                    web_url.as_deref(),
                ).await {
                    // Per-iteration error must not crash the loop. The
                    // restart=on-failure systemd unit is a coarse safety
                    // net but flapping under transient Anthropic errors
                    // would be hostile.
                    tracing::warn!(error = ?e, "poll iteration failed; will retry next tick");
                }
            }
        }
    }

    tracing::info!("skill-pool capturer daemon stopped");
    Ok(())
}

/// One pass over the queue. Picks up to `batch` candidates and runs each
/// through the shared `process_one` pipeline. Marker files are cleaned
/// up after a successful draft; on failure they're renamed to
/// `<id>.failed-<ts>` so the next pass doesn't retry forever.
async fn poll_once<S>(
    stages: &S,
    registry: &Client,
    batch: usize,
    allow_secret: bool,
    notify_enabled: bool,
    web_url: Option<&str>,
) -> Result<()>
where
    S: skill_pool_cli::capturer::Stages,
{
    let candidates = gather_candidates(batch)?;
    if candidates.is_empty() {
        tracing::debug!("no new candidates this tick");
        return Ok(());
    }
    tracing::info!(count = candidates.len(), "processing candidates");

    for cand in candidates {
        let result = process_one(
            stages,
            registry,
            &cand.score,
            find_transcript_for_session,
            allow_secret,
            notify_enabled,
            web_url,
        )
        .await;
        match result {
            Ok(state) => {
                // Persist the new state so the next poll skips this
                // session (idempotency via `capture_state`).
                let updated = SessionScore {
                    capture_state: Some(state),
                    ..cand.score
                };
                if let Err(e) = scorer::save_score(&updated) {
                    tracing::warn!(error = ?e, "persist capture_state failed");
                }
                // Drop the marker file if there was one — the score
                // record is now the source of truth.
                if let Some(marker) = cand.marker.as_ref() {
                    if let Err(e) = std::fs::remove_file(marker) {
                        tracing::warn!(error = ?e, marker = %marker.display(), "remove marker failed");
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    session = %cand.score.session_id,
                    error = ?e,
                    "capturer error; will not persist state"
                );
                // Rename the marker so we don't retry forever on a bad
                // session. The score's `capture_state` stays None
                // (process_one returns Err only for non-recoverable
                // infra errors, not pipeline rejection), so a future
                // operator can flip the marker back manually.
                if let Some(marker) = cand.marker.as_ref() {
                    let mut failed = marker.clone();
                    let ts = chrono::Utc::now().format("%Y%m%dT%H%M%S");
                    failed.set_extension(format!("failed-{ts}"));
                    if let Err(e) = std::fs::rename(marker, &failed) {
                        tracing::warn!(
                            error = ?e,
                            from = %marker.display(),
                            to = %failed.display(),
                            "rename failed marker"
                        );
                    }
                }
            }
        }
    }
    Ok(())
}

/// Pair a score with its (optional) queue-marker path. The marker is
/// the trigger; the score is what the pipeline operates on. Slice A's
/// SessionEnd hook writes the marker; we walk back to the score by
/// session_id.
struct Candidate {
    score: SessionScore,
    /// If a queue marker was the source, we'll clean it up on success.
    /// None when the candidate was found purely by scanning sessions/.
    marker: Option<PathBuf>,
}

/// Find up to `batch` fresh candidates. Order:
///  1. Queue markers (Slice A path) — these are explicit triggers.
///  2. Score scan fallback — picks up sessions whose Stop hook scored
///     them above threshold even without a SessionEnd marker.
fn gather_candidates(batch: usize) -> Result<Vec<Candidate>> {
    let batch = batch.max(1);
    let mut out: Vec<Candidate> = Vec::new();

    // ---- (1) queue markers ----
    let queue_dir = queue_dir()?;
    if queue_dir.exists() {
        for entry in std::fs::read_dir(&queue_dir)
            .with_context(|| format!("read_dir {}", queue_dir.display()))?
        {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("queued") {
                continue;
            }
            // Marker filename convention: `<session_id>.queued`. Slice A
            // hasn't formally specified the body yet; we only need the
            // session_id, so we take it from the stem.
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let session_id = stem.to_string();
            match load_score_for(&session_id) {
                Ok(Some(score)) if score.capture_state.is_none() => {
                    out.push(Candidate {
                        score,
                        marker: Some(path),
                    });
                }
                Ok(Some(_)) => {
                    // Already processed — silently drop the marker so we
                    // don't keep re-checking it.
                    if let Err(e) = std::fs::remove_file(&path) {
                        tracing::warn!(error = ?e, "remove stale marker");
                    }
                }
                Ok(None) => {
                    tracing::debug!(session = %session_id, "queue marker without score; ignoring");
                }
                Err(e) => {
                    tracing::warn!(session = %session_id, error = ?e, "load score for marker");
                }
            }
            if out.len() >= batch {
                return Ok(out);
            }
        }
    }

    // ---- (2) score-scan fallback ----
    if out.len() < batch {
        let scores = scorer::load_all_scores().context("scan sessions/")?;
        for score in scores.into_iter().filter(scorer::needs_capturing) {
            // Avoid double-queueing if the marker pass already enqueued
            // this session.
            if out.iter().any(|c| c.score.session_id == score.session_id) {
                continue;
            }
            out.push(Candidate {
                score,
                marker: None,
            });
            if out.len() >= batch {
                break;
            }
        }
    }

    Ok(out)
}

fn queue_dir() -> Result<PathBuf> {
    // Mirrors `scorer::sessions_dir` but with the `queue/` leaf instead
    // of `sessions/`. Slice A will write markers here.
    if let Ok(dir) = std::env::var("SKILL_POOL_HOME") {
        return Ok(PathBuf::from(dir).join("queue"));
    }
    let base = match std::env::var("XDG_DATA_HOME") {
        Ok(s) if !s.is_empty() => PathBuf::from(s).join("skill-pool"),
        _ => {
            let home = std::env::var("HOME").context("HOME not set")?;
            PathBuf::from(home).join(".skill-pool")
        }
    };
    Ok(base.join("queue"))
}

fn load_score_for(session_id: &str) -> Result<Option<SessionScore>> {
    let dir = scorer::sessions_dir()?;
    let path = dir.join(format!("{}.json", sanitize_session_id(session_id)));
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let score: SessionScore = serde_json::from_str(&raw)
        .with_context(|| format!("parse {}", path.display()))?;
    Ok(Some(score))
}

/// Sanitiser mirror of `scorer::save_score_in`'s private helper — kept
/// in sync so a marker's session_id resolves to the same on-disk score
/// filename Scorer wrote.
fn sanitize_session_id(id: &str) -> String {
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
