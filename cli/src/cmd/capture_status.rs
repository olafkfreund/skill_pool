//! `skill-pool capture-status` — read every persisted session score and
//! print a ranked summary. The `--json` form dumps the raw records for
//! piping into the (later) capturer daemon.

use anyhow::Result;

use crate::scorer::{self, SessionScore, DRAFT_THRESHOLD};

pub fn run(json: bool) -> Result<()> {
    let scores = scorer::load_all_scores()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&scores)?);
        return Ok(());
    }

    if scores.is_empty() {
        println!("(no session scores yet — wire the Stop hook with `skill-pool hook-install`)");
        println!("  sessions dir: {}", scorer::sessions_dir()?.display());
        return Ok(());
    }

    let draft_worthy: Vec<&SessionScore> = scores.iter().filter(|s| s.score >= DRAFT_THRESHOLD).collect();
    println!(
        "{} session{} scored ({} ≥ draft threshold of {})",
        scores.len(),
        if scores.len() == 1 { "" } else { "s" },
        draft_worthy.len(),
        DRAFT_THRESHOLD,
    );
    println!();
    println!("  {:<5} {:<14} {:<40} SESSION", "SCORE", "TURNS", "CWD");
    for s in &scores {
        let cwd = s.cwd.as_deref().unwrap_or("—");
        let marker = if s.score >= DRAFT_THRESHOLD { "★" } else { " " };
        println!(
            "  {marker}{:<4} {:<14} {:<40} {}",
            s.score,
            s.turn_count,
            truncate_mid(cwd, 40),
            short_id(&s.session_id),
        );
        // Surface the strongest signal one line below for context.
        if let Some(strongest) = s.signals.iter().max_by_key(|sig| sig.weight) {
            println!(
                "        ↳ {}: {}",
                serde_kind(&strongest.kind),
                strongest.evidence
            );
        }
    }
    if !draft_worthy.is_empty() {
        println!();
        println!(
            "★ marks sessions at or above the draft threshold ({}). Run \
             `skill-pool capture <DIR>` to push one as a draft.",
            DRAFT_THRESHOLD
        );
    }
    Ok(())
}

fn short_id(id: &str) -> String {
    if id.len() <= 12 {
        id.to_string()
    } else {
        format!("{}…", &id[..12])
    }
}

fn truncate_mid(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let tail = keep / 2;
    let head = keep - tail;
    format!("{}…{}", &s[..head], &s[s.len() - tail..])
}

fn serde_kind(k: &scorer::SignalKind) -> &'static str {
    match k {
        scorer::SignalKind::ExplicitMarker => "explicit_marker",
        scorer::SignalKind::TestRecovery => "test_recovery",
        scorer::SignalKind::EditRetry => "edit_retry",
        scorer::SignalKind::LongSession => "long_session",
    }
}
