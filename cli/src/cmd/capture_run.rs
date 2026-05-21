//! `skill-pool capture-run` — Phase 4.6 LLM capturer pipeline.
//!
//! Idempotent: each session is processed once. The outcome lands in the
//! score record's `capture_state`, so cron-driven re-invocations skip
//! sessions that have already been handled (drafted, rejected, or failed).
//!
//! Pipeline per session:
//!   1. Load transcript at `cwd_hint` resolution from Claude Code's
//!      `.claude/projects/` tree (best-effort; we can't always find it).
//!   2. Stage 1 — Haiku: returns JSON; reject if `generalizable: false`.
//!   3. Stage 2 — Sonnet: returns SKILL.md.
//!   4. Client-side validate. Build bundle. POST to /v1/drafts.
//!   5. Persist updated score with `capture_state`.
//!
//! Designed to be invoked by a systemd user timer (or cron). Exits 0 on
//! "nothing to do" and on per-session errors — the cron must not flap.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use chrono::Utc;

use crate::anthropic::AnthropicClient;
use crate::capturer::{self, AnthropicStages, Stages, DEFAULT_STAGE1_MODEL, DEFAULT_STAGE2_MODEL};
use crate::client::{CaptureMetadata, CapturedDraft, Client};
use crate::config::Config;
use crate::notify;
use crate::scorer::{self, CaptureStage, CaptureState, SessionScore};
use crate::secret_scan;

/// Server submit seam. Production uses `Client`; tests inject a stub that
/// captures the call without an HTTP round-trip.
#[allow(async_fn_in_trait)]
pub trait DraftSubmit {
    async fn submit_draft<'a>(
        &self,
        metadata: CaptureMetadata<'a>,
        bundle: Bytes,
    ) -> Result<CapturedDraft>;
}

impl DraftSubmit for Client {
    async fn submit_draft<'a>(
        &self,
        metadata: CaptureMetadata<'a>,
        bundle: Bytes,
    ) -> Result<CapturedDraft> {
        Client::submit_draft(self, metadata, bundle).await
    }
}

pub async fn run(
    cfg: &Config,
    limit: usize,
    dry_run: bool,
    stage1_model: Option<&str>,
    stage2_model: Option<&str>,
    allow_secret: bool,
    no_notify: bool,
) -> Result<()> {
    let stage1 = stage1_model.unwrap_or(DEFAULT_STAGE1_MODEL);
    let stage2 = stage2_model.unwrap_or(DEFAULT_STAGE2_MODEL);

    let all = scorer::load_all_scores().context("load session scores")?;
    let work: Vec<SessionScore> = all
        .into_iter()
        .filter(scorer::needs_capturing)
        .take(limit.max(1))
        .collect();

    if work.is_empty() {
        println!("(no draft-worthy unprocessed sessions)");
        return Ok(());
    }

    println!(
        "{} candidate session{} (limit {}); models: stage1={} stage2={}",
        work.len(),
        if work.len() == 1 { "" } else { "s" },
        limit,
        stage1,
        stage2,
    );

    if dry_run {
        for s in &work {
            println!(
                "  · {} (score={}, turns={}, cwd={})",
                short(&s.session_id),
                s.score,
                s.turn_count,
                s.cwd.as_deref().unwrap_or("—"),
            );
        }
        println!("(dry-run; no LLM calls, no draft POST)");
        return Ok(());
    }

    let llm = AnthropicClient::from_env()?;
    let stages = AnthropicStages {
        client: &llm,
        stage1_model: stage1,
        stage2_model: stage2,
    };
    let reg = cfg.require_registry()?;
    let registry = Client::new(reg)?;
    let notify_enabled = !no_notify;
    let web_url = cfg.web_url.as_deref();

    let mut drafted = 0;
    let mut rejected = 0;
    let mut errored = 0;

    for s in work {
        match process_one(
            &stages,
            &registry,
            &s,
            find_transcript_for_session,
            allow_secret,
            notify_enabled,
            web_url,
        )
        .await
        {
            Ok(state) => {
                let drafted_now = matches!(state.stage, CaptureStage::Drafted);
                let rejected_now = matches!(
                    state.stage,
                    CaptureStage::Stage1Rejected
                        | CaptureStage::Stage2Rejected
                        | CaptureStage::Stage1ParseFailure
                );
                if drafted_now {
                    drafted += 1;
                } else if rejected_now {
                    rejected += 1;
                }
                let updated = SessionScore {
                    capture_state: Some(state),
                    ..s
                };
                if let Err(e) = scorer::save_score(&updated) {
                    tracing::warn!(error = ?e, "persist capture_state failed");
                }
            }
            Err(e) => {
                errored += 1;
                tracing::warn!(session = %s.session_id, error = ?e, "capturer error");
                // Don't persist a state — we'll retry next pass. The cron
                // shouldn't flap, but we also don't want to bury a
                // recoverable error under a permanent ServerRejected.
            }
        }
    }

    println!();
    println!("summary: {drafted} drafted, {rejected} rejected by pipeline, {errored} errored");
    Ok(())
}

/// Process a single session through the Phase 4.6 pipeline.
///
/// Returns the persisted `CaptureState` so the caller can record it in
/// the session's score file. Fires a desktop notification on the
/// `Drafted` outcome when `notify_enabled` is true and the host has a
/// session bus (see `crate::notify::should_emit`).
///
/// Re-used by the long-lived daemon binary so the two drivers share one
/// pipeline implementation.
pub async fn process_one<S, D, F>(
    stages: &S,
    registry: &D,
    s: &SessionScore,
    resolve_transcript: F,
    allow_secret: bool,
    notify_enabled: bool,
    web_url: Option<&str>,
) -> Result<CaptureState>
where
    S: Stages,
    D: DraftSubmit,
    F: Fn(&SessionScore) -> Result<std::path::PathBuf>,
{
    // 1. Find + load the transcript.
    let transcript_path = resolve_transcript(s)?;
    let raw_jsonl = std::fs::read_to_string(&transcript_path)
        .with_context(|| format!("read transcript {}", transcript_path.display()))?;
    let events = scorer::parse_transcript(&raw_jsonl);
    let trimmed = capturer::trim_transcript(&events);
    if trimmed.trim().is_empty() {
        return Ok(CaptureState {
            stage: CaptureStage::Stage1Rejected,
            completed_at: Utc::now(),
            draft_id: None,
            slug: None,
            reason: Some("transcript produced no scorable events".into()),
        });
    }

    // 2. Stage 1 — extractor.
    println!("  [{}] stage1…", short(&s.session_id));
    let stage1 = match stages.stage1(&trimmed).await {
        Ok(a) => a,
        Err(e) => {
            return Ok(CaptureState {
                stage: CaptureStage::Stage1ParseFailure,
                completed_at: Utc::now(),
                draft_id: None,
                slug: None,
                reason: Some(format!("stage1 failed: {e}")),
            });
        }
    };
    if !stage1.generalizable {
        let reason = stage1
            .reason
            .clone()
            .unwrap_or_else(|| "stage1 said not generalizable".into());
        println!("    rejected: {reason}");
        return Ok(CaptureState {
            stage: CaptureStage::Stage1Rejected,
            completed_at: Utc::now(),
            draft_id: None,
            slug: None,
            reason: Some(reason),
        });
    }

    // 2b. Pre-stage-2 secret scan. A transcript dense with credentials is
    // unlikely to produce a clean draft anyway; bail before we burn the
    // Sonnet call. `--allow-secret` downgrades this to a warning.
    let pre_findings = secret_scan::scan_text(&trimmed);
    if !pre_findings.is_empty() {
        let summary = secret_scan::summarise(&pre_findings);
        if allow_secret {
            tracing::warn!(
                session = %s.session_id,
                "pre-stage-2 secret findings (proceeding under --allow-secret): {summary}",
            );
            println!(
                "    ! pre-stage-2 secrets found ({} finding{}); proceeding under --allow-secret",
                pre_findings.len(),
                if pre_findings.len() == 1 { "" } else { "s" },
            );
        } else {
            println!(
                "    skipping session: {} secret finding{} in transcript",
                pre_findings.len(),
                if pre_findings.len() == 1 { "" } else { "s" },
            );
            return Ok(CaptureState {
                stage: CaptureStage::Stage1Rejected,
                completed_at: Utc::now(),
                draft_id: None,
                slug: None,
                reason: Some(format!("transcript contained secrets: {summary}")),
            });
        }
    }

    // 3. Stage 2 — drafter.
    println!("  [{}] stage2…", short(&s.session_id));
    let md = stages.stage2(&stage1, &trimmed).await?;
    let validated = match capturer::validate_skill_md(&md) {
        Ok(v) => v,
        Err(e) => {
            return Ok(CaptureState {
                stage: CaptureStage::Stage2Rejected,
                completed_at: Utc::now(),
                draft_id: None,
                slug: None,
                reason: Some(format!("validation failed: {e}")),
            });
        }
    };

    // 4. Bundle + POST.
    let bundle = capturer::bundle_skill_md(&md)?;

    // 4a. Pre-POST secret scan. Sonnet can introduce strings that were
    // never in the transcript (hallucinated tokens, mirrored values), so
    // the bundle scan is the real safety net.
    let bundle_findings =
        secret_scan::scan_bundle(&bundle).context("scan generated bundle for secrets")?;
    if !bundle_findings.is_empty() {
        let summary = secret_scan::summarise(&bundle_findings);
        if allow_secret {
            tracing::warn!(
                session = %s.session_id,
                "pre-POST secret findings (proceeding under --allow-secret): {summary}",
            );
            println!(
                "    ! pre-POST secrets found ({} finding{}); proceeding under --allow-secret",
                bundle_findings.len(),
                if bundle_findings.len() == 1 { "" } else { "s" },
            );
        } else {
            println!(
                "    skipping POST: {} secret finding{} in bundle",
                bundle_findings.len(),
                if bundle_findings.len() == 1 { "" } else { "s" },
            );
            return Ok(CaptureState {
                stage: CaptureStage::Stage2Rejected,
                completed_at: Utc::now(),
                draft_id: None,
                slug: None,
                reason: Some(format!("bundle contained secrets: {summary}")),
            });
        }
    }

    let notes = build_capture_notes(s, &stage1);
    let metadata = CaptureMetadata {
        slug: &validated.slug,
        origin: "capture-scorer",
        notes: Some(&notes),
        tags: &validated.tags,
        when_to_use: validated.when_to_use.as_deref(),
    };
    match registry.submit_draft(metadata, bundle).await {
        Ok(draft) => {
            println!("    → drafted {} ({})", draft.slug, short(&draft.id));
            // Desktop toast — gated on the caller's `--no-notify` and
            // on the host actually having a session bus. Headless boxes
            // (systemd daemon with no DBUS env, CI) silently skip.
            notify::notify_draft_ready(notify_enabled, &draft.slug, &draft.id, web_url);
            Ok(CaptureState {
                stage: CaptureStage::Drafted,
                completed_at: Utc::now(),
                draft_id: Some(draft.id),
                slug: Some(draft.slug),
                reason: None,
            })
        }
        Err(e) => Ok(CaptureState {
            stage: CaptureStage::ServerRejected,
            completed_at: Utc::now(),
            draft_id: None,
            slug: Some(validated.slug),
            reason: Some(format!("server rejected: {e}")),
        }),
    }
}

fn build_capture_notes(s: &SessionScore, stage1: &capturer::Stage1Analysis) -> String {
    let mut n = format!(
        "Captured from session {} (score {}). ",
        short(&s.session_id),
        s.score
    );
    if let Some(cwd) = &s.cwd {
        n.push_str(&format!("cwd: {cwd}. "));
    }
    let signals: Vec<_> = s.signals.iter().map(|sig| sig.evidence.as_str()).collect();
    if !signals.is_empty() {
        n.push_str(&format!("Signals: {}. ", signals.join("; ")));
    }
    if let Some(reason) = &stage1.reason {
        n.push_str(&format!("Stage1: {reason}"));
    }
    n
}

/// Resolve the transcript file path. Claude Code stores transcripts under
/// `<home>/.claude/projects/<encoded-cwd>/<session-id>.jsonl`. Walks the
/// `.claude/projects` tree under `home_root` looking for the matching file.
pub(crate) fn find_transcript_in(
    home_root: &std::path::Path,
    session_id: &str,
) -> Result<std::path::PathBuf> {
    let projects = home_root.join(".claude").join("projects");
    if !projects.exists() {
        return Err(anyhow!(
            "Claude Code projects dir not found at {}",
            projects.display()
        ));
    }
    let needle = format!("{session_id}.jsonl");
    for entry in walkdir::WalkDir::new(&projects)
        .max_depth(4)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if entry.file_name().to_string_lossy() == needle {
            return Ok(entry.into_path());
        }
    }
    Err(anyhow!(
        "transcript {} not found under {}",
        needle,
        projects.display()
    ))
}

pub fn find_transcript_for_session(s: &SessionScore) -> Result<std::path::PathBuf> {
    let home = std::env::var("HOME").context("HOME not set")?;
    find_transcript_in(std::path::Path::new(&home), &s.session_id)
}

fn short(id: &str) -> String {
    if id.len() <= 8 {
        id.to_string()
    } else {
        id[..8].to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests — exercise the orchestrator's decision tree with stubbed stages and
// a stubbed draft submitter. No HTTP, no Anthropic.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capturer::Stage1Analysis;
    use crate::scorer::{ScoreBreakdown, Signal, SignalKind};
    use std::sync::Mutex;

    /// Programmable Stage1/Stage2 stub. Each call advances the next item
    /// from the configured queues.
    struct StubStages {
        stage1: Mutex<Vec<Result<Stage1Analysis>>>,
        stage2: Mutex<Vec<Result<String>>>,
    }

    impl Stages for StubStages {
        async fn stage1(&self, _transcript: &str) -> Result<Stage1Analysis> {
            self.stage1.lock().unwrap().remove(0)
        }
        async fn stage2(&self, _a: &Stage1Analysis, _t: &str) -> Result<String> {
            self.stage2.lock().unwrap().remove(0)
        }
    }

    /// DraftSubmit stub: records calls and returns a canned response, or
    /// returns an error if `fail` was set.
    struct StubSubmit {
        calls: Mutex<Vec<String>>,
        fail: bool,
    }

    impl DraftSubmit for StubSubmit {
        async fn submit_draft<'a>(
            &self,
            metadata: CaptureMetadata<'a>,
            _bundle: Bytes,
        ) -> Result<CapturedDraft> {
            self.calls.lock().unwrap().push(metadata.slug.to_string());
            if self.fail {
                Err(anyhow!("stub: server rejected"))
            } else {
                Ok(CapturedDraft {
                    id: "stub-draft-id".into(),
                    slug: metadata.slug.to_string(),
                    status: "pending".into(),
                })
            }
        }
    }

    fn make_session(id: &str) -> SessionScore {
        SessionScore {
            session_id: id.into(),
            cwd: Some("/proj".into()),
            score: 1050,
            breakdown: ScoreBreakdown::default(),
            signals: vec![Signal {
                kind: SignalKind::ExplicitMarker,
                weight: 1000,
                evidence: "user said `remember this`".into(),
            }],
            turn_count: 5,
            last_scored_at: chrono::Utc::now(),
            version: 2,
            capture_state: None,
        }
    }

    /// Build a tempdir laid out like `~/.claude/projects/<dir>/<id>.jsonl`
    /// and write a synthetic transcript inside it. Returns the resolver
    /// closure pointing at it.
    fn fixture_transcript(
        jsonl: &str,
        session_id: &str,
    ) -> (
        tempfile::TempDir,
        impl Fn(&SessionScore) -> Result<std::path::PathBuf>,
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let proj_dir = tmp.path().join(".claude").join("projects").join("dummy");
        std::fs::create_dir_all(&proj_dir).unwrap();
        let transcript_path = proj_dir.join(format!("{session_id}.jsonl"));
        std::fs::write(&transcript_path, jsonl).unwrap();
        let home = tmp.path().to_path_buf();
        let resolver = move |s: &SessionScore| -> Result<std::path::PathBuf> {
            find_transcript_in(&home, &s.session_id)
        };
        (tmp, resolver)
    }

    const TINY_JSONL: &str = r#"{"type":"user","message":{"content":"please remember this for next time"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}
"#;

    fn ok_analysis(g: bool) -> Stage1Analysis {
        Stage1Analysis {
            problem: "p".into(),
            solution_steps: vec!["s".into()],
            generalizable: g,
            scope: Some("tool".into()),
            preconditions: vec![],
            reason: Some(if g {
                "ok".into()
            } else {
                "too specific".into()
            }),
        }
    }

    fn valid_md() -> String {
        "---\nname: foo\ndescription: A captured pattern.\ntags: [test]\n---\n\n# foo\n\nsteps.\n"
            .into()
    }

    #[tokio::test]
    async fn stage1_rejection_records_state_and_skips_stage2() {
        let s = make_session("s-rej");
        let (_tmp, resolver) = fixture_transcript(TINY_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(false))]),
            stage2: Mutex::new(vec![]), // would panic if Stage 2 ran
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Stage1Rejected));
        assert!(state.reason.unwrap().contains("too specific"));
        assert!(
            submit.calls.lock().unwrap().is_empty(),
            "no draft POST expected"
        );
    }

    #[tokio::test]
    async fn stage1_parse_failure_yields_dedicated_state() {
        let s = make_session("s-parse");
        let (_tmp, resolver) = fixture_transcript(TINY_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Err(anyhow!("malformed JSON from model"))]),
            stage2: Mutex::new(vec![]),
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Stage1ParseFailure));
        assert!(state.reason.unwrap().contains("malformed JSON"));
    }

    #[tokio::test]
    async fn stage2_validation_failure_records_stage2_rejected() {
        let s = make_session("s-2rej");
        let (_tmp, resolver) = fixture_transcript(TINY_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(true))]),
            // Stage 2 returns markdown with a /home/ path → server-style rejection.
            stage2: Mutex::new(vec![Ok(
                "---\nname: foo\ndescription: bad.\n---\n\nrun /home/alice/scripts\n".to_string(),
            )]),
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Stage2Rejected));
        assert!(state
            .reason
            .unwrap()
            .to_lowercase()
            .contains("absolute path"));
        assert!(
            submit.calls.lock().unwrap().is_empty(),
            "no POST after Stage2 reject"
        );
    }

    #[tokio::test]
    async fn happy_path_drafts_through_to_server() {
        let s = make_session("s-ok");
        let (_tmp, resolver) = fixture_transcript(TINY_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(true))]),
            stage2: Mutex::new(vec![Ok(valid_md())]),
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Drafted));
        assert_eq!(state.draft_id.as_deref(), Some("stub-draft-id"));
        assert_eq!(state.slug.as_deref(), Some("foo"));
        let calls = submit.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0], "foo");
    }

    #[tokio::test]
    async fn server_failure_records_server_rejected() {
        let s = make_session("s-srv");
        let (_tmp, resolver) = fixture_transcript(TINY_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(true))]),
            stage2: Mutex::new(vec![Ok(valid_md())]),
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: true,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::ServerRejected));
        assert!(state.reason.unwrap().contains("server rejected"));
    }

    /// A transcript whose user message includes a credential should short-
    /// circuit at the pre-stage-2 gate, recording Stage1Rejected with a
    /// secrets reason. Crucially, Stage 2 must NOT run (queue stays full).
    const SECRET_JSONL: &str = r#"{"type":"user","message":{"content":"my key is AKIAIOSFODNN7EXAMPLE please help"}}
{"type":"assistant","message":{"content":[{"type":"text","text":"ok"}]}}
"#;

    #[tokio::test]
    async fn pre_stage2_secret_skips_session_and_does_not_call_stage2() {
        let s = make_session("s-secret-transcript");
        let (_tmp, resolver) = fixture_transcript(SECRET_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(true))]),
            stage2: Mutex::new(vec![]), // panics if Stage 2 runs
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Stage1Rejected));
        let reason = state.reason.unwrap();
        assert!(reason.contains("secrets"), "reason: {reason}");
        assert!(reason.contains("aws-access-key"), "reason: {reason}");
        assert!(submit.calls.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn allow_secret_overrides_pre_stage2_gate() {
        let s = make_session("s-secret-allow");
        let (_tmp, resolver) = fixture_transcript(SECRET_JSONL, &s.session_id);
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(true))]),
            stage2: Mutex::new(vec![Ok(valid_md())]),
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        // allow_secret = true → pipeline should proceed past the gate.
        let state = process_one(&stages, &submit, &s, resolver, true, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Drafted));
    }

    /// Stage 2 may "hallucinate" a token that wasn't in the transcript and
    /// that the narrow `validate_skill_md` rules don't catch. The pre-POST
    /// bundle scan is the safety net for those cases.
    #[tokio::test]
    async fn pre_post_bundle_secret_blocks_submit() {
        let s = make_session("s-secret-bundle");
        let (_tmp, resolver) = fixture_transcript(TINY_JSONL, &s.session_id);
        // A Slack token: validate_skill_md doesn't know about it, but
        // secret_scan does. This isolates the pre-POST gate.
        let md_with_slack = "---\nname: foo\ndescription: A captured pattern.\n---\n\n# foo\n\nuse SLACK=xoxb-12345-abcdef-67890-zzzaaa\n";
        let stages = StubStages {
            stage1: Mutex::new(vec![Ok(ok_analysis(true))]),
            stage2: Mutex::new(vec![Ok(md_with_slack.to_string())]),
        };
        let submit = StubSubmit {
            calls: Mutex::new(vec![]),
            fail: false,
        };
        let state = process_one(&stages, &submit, &s, resolver, false, false, None)
            .await
            .unwrap();
        assert!(matches!(state.stage, CaptureStage::Stage2Rejected));
        let reason = state.reason.unwrap();
        assert!(
            reason.contains("bundle contained secrets"),
            "reason: {reason}"
        );
        assert!(reason.contains("slack-token"), "reason: {reason}");
        assert!(submit.calls.lock().unwrap().is_empty());
    }
}
