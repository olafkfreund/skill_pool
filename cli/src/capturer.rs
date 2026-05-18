//! Phase 4.6 capturer pipeline.
//!
//! Two-stage LLM pass that turns a draft-worthy session into a SKILL.md
//! draft:
//!
//!  1. **Stage 1 — extractor (Haiku)**: cheap, returns strict JSON that
//!     answers "is this generalizable?" Tunable rejection here so we don't
//!     burn Sonnet on project-specific noise (~70% of sessions stop here).
//!  2. **Stage 2 — drafter (Sonnet)**: only runs when Stage 1 approves.
//!     Produces raw SKILL.md content from the Stage 1 analysis + the
//!     trimmed transcript.
//!
//! This module is pure orchestration. The CLI command in
//! `cmd::capture_run` ties it to the score store, the Anthropic client,
//! and the draft POST.

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::anthropic::{AnthropicClient, CreateMessageRequest, Message};

/// Default model for the cheap extractor pass.
pub const DEFAULT_STAGE1_MODEL: &str = "claude-haiku-4-5-20251001";

/// Default model for the more expensive drafter pass.
pub const DEFAULT_STAGE2_MODEL: &str = "claude-sonnet-4-6";

/// Cap on transcript chars we send. Sonnet handles much more but Stage 1 is
/// the gate and we don't want to balloon Haiku costs. ~3000 tokens equiv.
pub const TRANSCRIPT_CHAR_BUDGET: usize = 12_000;

const STAGE1_SYSTEM: &str = "You are evaluating whether a Claude Code session contains a generalizable, \
reusable lesson worth saving as a team skill.\n\
\n\
Reply with EXACTLY one JSON object — no prose, no markdown fences, no preamble. \
Schema:\n\
{\n  \"problem\": \"<one-sentence problem statement>\",\n  \"solution_steps\": [\"<step>\"],\n  \"generalizable\": <true|false>,\n  \"scope\": \"language\" | \"framework\" | \"tool\" | \"general\",\n  \"preconditions\": [\"<setup that must hold>\"],\n  \"reason\": \"<why you set generalizable as you did>\"\n}\n\
\n\
Set `generalizable: false` when the session is:\n\
- project-specific config or one-off debugging,\n\
- a question with no actionable answer,\n\
- mostly file reads with no insight,\n\
- routine code edits without a learned pattern.\n\
\n\
Set `generalizable: true` only when another developer hitting a similar problem \
would benefit from this pattern.";

const STAGE2_SYSTEM: &str = "You are drafting a Claude Code skill (a SKILL.md file).\n\
\n\
Output ONLY the SKILL.md content — no commentary, no fences around the file. \
Begin with `---` (YAML frontmatter), end with the body.\n\
\n\
Frontmatter schema:\n\
---\n\
name: <slug-style-name>             # lowercase, hyphens, no spaces\n\
description: <1-2 sentences>        # third-person present tense, < 1500 chars\n\
when_to_use: <invocation hint>      # under 240 chars\n\
tags: [<tag1>, <tag2>]              # lowercase, hyphenated\n\
---\n\
\n\
Then a short body — concise steps, code blocks where they help. The body \
must be self-contained: the original session won't be available to a future \
reader.\n\
\n\
Constraints (CRITICAL — the server will reject the draft otherwise):\n\
- No absolute paths like /home/<user>/ or /Users/<user>/ — they identify the \
  author and break on other machines.\n\
- No real credentials, API tokens, or private URLs.\n\
- The description must be 1-2 sentences (not a heading or a question).";

/// Stage 1 output. Deserialised from the LLM's JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stage1Analysis {
    pub problem: String,
    #[serde(default)]
    pub solution_steps: Vec<String>,
    pub generalizable: bool,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub preconditions: Vec<String>,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Cap a transcript projection at `TRANSCRIPT_CHAR_BUDGET` chars by keeping
/// the most recent material (the tail is where the resolution lives) plus
/// the first user message (states the problem).
pub fn trim_transcript(events: &[crate::scorer::Event]) -> String {
    let mut full = String::with_capacity(8 * 1024);
    for ev in events {
        match ev {
            crate::scorer::Event::UserText(s) => {
                full.push_str("USER: ");
                full.push_str(s);
                full.push('\n');
            }
            crate::scorer::Event::AssistantText => {
                full.push_str("ASSISTANT: (text reply)\n");
            }
            crate::scorer::Event::ToolUse { name, target } => {
                let t = target.as_deref().unwrap_or("");
                full.push_str(&format!("TOOL_USE {name}({t})\n"));
            }
            crate::scorer::Event::ToolResult { is_error, body } => {
                let tag = if *is_error { "TOOL_ERR" } else { "TOOL_OK" };
                full.push_str(tag);
                full.push_str(": ");
                full.push_str(truncate(body, 400));
                full.push('\n');
            }
        }
    }
    if full.len() <= TRANSCRIPT_CHAR_BUDGET {
        return full;
    }
    // Keep the first ~1500 chars (problem statement usually lives there)
    // and the last ~(budget - 1500 - marker) chars (resolution lives there).
    const MARKER: &str = "\n\n[... transcript truncated for length ...]\n\n";
    let head_keep = 1500.min(TRANSCRIPT_CHAR_BUDGET / 4);
    let tail_keep = TRANSCRIPT_CHAR_BUDGET
        .saturating_sub(head_keep)
        .saturating_sub(MARKER.len());
    let head = safe_slice(&full, 0, head_keep);
    let tail = safe_slice(&full, full.len() - tail_keep, full.len());
    format!("{head}{MARKER}{tail}")
}

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let mut end = max;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        &s[..end]
    }
}

fn safe_slice(s: &str, start: usize, end: usize) -> &str {
    let mut a = start.min(s.len());
    let mut b = end.min(s.len());
    while !s.is_char_boundary(a) && a < s.len() {
        a += 1;
    }
    while !s.is_char_boundary(b) && b > 0 {
        b -= 1;
    }
    if a > b {
        a = b;
    }
    &s[a..b]
}

/// Assemble the user-side prompt for Stage 1 (just the transcript wrapped
/// in delimiters). Kept as a pure function so prompts can be snapshot-tested.
pub fn stage1_user_prompt(transcript: &str) -> String {
    format!(
        "Transcript:\n<<<\n{}\n>>>\n\nReturn ONLY the JSON object — no prose.",
        transcript
    )
}

/// Same for Stage 2 — embeds the Stage 1 verdict so Sonnet has structured
/// context without re-deriving it.
pub fn stage2_user_prompt(analysis: &Stage1Analysis, transcript: &str) -> String {
    let analysis_json = serde_json::to_string_pretty(analysis)
        .unwrap_or_else(|_| "{}".to_string());
    format!(
        "Stage 1 analysis:\n{}\n\nTranscript:\n<<<\n{}\n>>>\n\nWrite the SKILL.md now.",
        analysis_json, transcript
    )
}

/// Parse the model's JSON reply. Tolerant of accidental markdown fences
/// (some models wrap JSON in ```json ... ``` despite instructions).
pub fn parse_stage1(raw: &str) -> Result<Stage1Analysis> {
    let cleaned = strip_fences(raw.trim());
    serde_json::from_str(cleaned)
        .with_context(|| format!("parse Stage 1 JSON: {}", truncate(cleaned, 200)))
}

fn strip_fences(s: &str) -> &str {
    let s = s
        .strip_prefix("```json")
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .trim();
    s.strip_suffix("```").unwrap_or(s).trim()
}

// --- LLM call wrappers ----------------------------------------------------

/// A pluggable interface for the two stages. Production uses
/// `AnthropicStages`; tests can inject a deterministic stub.
#[allow(async_fn_in_trait)] // single-crate trait, no Send-across-await constraint needed
pub trait Stages {
    async fn stage1(&self, transcript: &str) -> Result<Stage1Analysis>;
    async fn stage2(&self, analysis: &Stage1Analysis, transcript: &str) -> Result<String>;
}

pub struct AnthropicStages<'a> {
    pub client: &'a AnthropicClient,
    pub stage1_model: &'a str,
    pub stage2_model: &'a str,
}

impl<'a> Stages for AnthropicStages<'a> {
    async fn stage1(&self, transcript: &str) -> Result<Stage1Analysis> {
        run_stage1(self.client, self.stage1_model, transcript).await
    }
    async fn stage2(&self, analysis: &Stage1Analysis, transcript: &str) -> Result<String> {
        run_stage2(self.client, self.stage2_model, analysis, transcript).await
    }
}

pub async fn run_stage1(
    client: &AnthropicClient,
    model: &str,
    transcript: &str,
) -> Result<Stage1Analysis> {
    let user_msg = stage1_user_prompt(transcript);
    let raw = client
        .create_message(CreateMessageRequest {
            model,
            max_tokens: 1024,
            system: STAGE1_SYSTEM,
            messages: vec![Message {
                role: "user",
                content: &user_msg,
            }],
            temperature: Some(0.0),
        })
        .await?;
    parse_stage1(&raw)
}

pub async fn run_stage2(
    client: &AnthropicClient,
    model: &str,
    analysis: &Stage1Analysis,
    transcript: &str,
) -> Result<String> {
    let user_msg = stage2_user_prompt(analysis, transcript);
    let raw = client
        .create_message(CreateMessageRequest {
            model,
            max_tokens: 2048,
            system: STAGE2_SYSTEM,
            messages: vec![Message {
                role: "user",
                content: &user_msg,
            }],
            temperature: Some(0.2),
        })
        .await?;
    let cleaned = strip_md_fences(raw.trim());
    Ok(cleaned.to_string())
}

fn strip_md_fences(s: &str) -> &str {
    let s = s
        .strip_prefix("```markdown")
        .or_else(|| s.strip_prefix("```md"))
        .or_else(|| s.strip_prefix("```"))
        .unwrap_or(s)
        .trim();
    s.strip_suffix("```").unwrap_or(s).trim()
}

// --- Client-side validation -----------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("missing YAML frontmatter (expected `---` block)")]
    MissingFrontmatter,
    #[error("frontmatter missing required `description` field")]
    MissingDescription,
    #[error("description is empty or too long ({0} chars; max 1500)")]
    BadDescription(usize),
    #[error("absolute path in body: {0}")]
    AbsolutePath(String),
    #[error("possible secret: {0}")]
    Secret(&'static str),
}

#[derive(Debug, Clone, serde::Deserialize)]
struct LocalFrontmatter {
    #[serde(default)]
    name: Option<String>,
    description: String,
    #[serde(default)]
    when_to_use: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

/// Mirror of the server-side bundle validator, just enough to refuse
/// obvious failures before we waste a round-trip.
pub fn validate_skill_md(md: &str) -> Result<ValidatedSkill, ValidationError> {
    let rest = md
        .strip_prefix("---\n")
        .or_else(|| md.strip_prefix("---\r\n"))
        .ok_or(ValidationError::MissingFrontmatter)?;
    let end = rest
        .find("\n---\n")
        .or_else(|| rest.find("\n---\r\n"))
        .ok_or(ValidationError::MissingFrontmatter)?;
    let yaml = &rest[..end];
    let body = rest[end + 1..]
        .strip_prefix("---\n")
        .or_else(|| rest[end + 1..].strip_prefix("---\r\n"))
        .unwrap_or(&rest[end + 1..]);

    let fm: LocalFrontmatter = serde_yaml::from_str(yaml).map_err(|_| {
        ValidationError::MissingDescription
    })?;
    if fm.description.is_empty() || fm.description.len() > 1500 {
        return Err(ValidationError::BadDescription(fm.description.len()));
    }

    for pat in ["/home/", "/Users/", r"C:\Users\"] {
        if let Some(idx) = body.find(pat) {
            let start = idx.saturating_sub(8);
            let end = (idx + 32).min(body.len());
            return Err(ValidationError::AbsolutePath(body[start..end].to_string()));
        }
    }

    // Cheap secret patterns — the server runs a stricter regex pass and
    // will still reject if anything slips through.
    for (label, marker) in [
        ("AWS access key ID", "AKIA"),
        ("GitHub PAT", "ghp_"),
        ("GitHub OAuth", "gho_"),
        ("PEM private key", "-----BEGIN"),
    ] {
        if body.contains(marker) {
            return Err(ValidationError::Secret(label));
        }
    }

    Ok(ValidatedSkill {
        slug: fm
            .name
            .unwrap_or_else(|| "captured-skill".to_string())
            .trim()
            .to_lowercase()
            .replace(' ', "-"),
        description: fm.description,
        when_to_use: fm.when_to_use,
        tags: fm.tags,
    })
}

#[derive(Debug, Clone)]
pub struct ValidatedSkill {
    pub slug: String,
    /// Extracted for tests and future use; the server re-derives description
    /// from the bundle so we don't pass it through.
    #[allow(dead_code)]
    pub description: String,
    pub when_to_use: Option<String>,
    pub tags: Vec<String>,
}

// --- Bundle building ------------------------------------------------------

/// Build a single-file `.tar.gz` containing `SKILL.md` at the archive root.
/// Used by the capturer because it never has a directory on disk to tar —
/// the SKILL.md comes straight from Stage 2.
pub fn bundle_skill_md(md: &str) -> Result<Bytes> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;

    let body = md.as_bytes();
    let mut tar = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header
        .set_path("SKILL.md")
        .map_err(|e| anyhow!("tar header path: {e}"))?;
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, body)
        .map_err(|e| anyhow!("tar append: {e}"))?;
    let tar_bytes = tar.into_inner().map_err(|e| anyhow!("tar finish: {e}"))?;

    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).map_err(|e| anyhow!("gz write: {e}"))?;
    let gz_bytes = gz.finish().map_err(|e| anyhow!("gz finish: {e}"))?;
    Ok(Bytes::from(gz_bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scorer::Event;

    fn ev_user(s: &str) -> Event {
        Event::UserText(s.into())
    }
    fn ev_tool(name: &str, target: &str) -> Event {
        Event::ToolUse {
            name: name.into(),
            target: Some(target.into()),
        }
    }
    #[allow(dead_code)] // kept for future Stage 2 prompt evidence tests
    fn ev_err(body: &str) -> Event {
        Event::ToolResult {
            is_error: true,
            body: body.into(),
        }
    }

    #[test]
    fn trim_keeps_short_transcripts() {
        let s = trim_transcript(&[ev_user("hi"), ev_tool("Bash", "ls")]);
        assert!(s.contains("USER: hi"));
        assert!(s.contains("TOOL_USE Bash(ls)"));
        assert!(s.len() < TRANSCRIPT_CHAR_BUDGET);
    }

    #[test]
    fn trim_truncates_long_transcripts() {
        // Build a transcript way over budget.
        let huge: Vec<Event> = (0..1000)
            .map(|i| ev_user(&format!("repeat #{i} ").repeat(20)))
            .collect();
        let s = trim_transcript(&huge);
        assert!(s.len() <= TRANSCRIPT_CHAR_BUDGET, "len = {}", s.len());
        assert!(s.contains("transcript truncated"));
        // Head preserved: the very first repeat is in there.
        assert!(s.contains("repeat #0"));
        // Tail preserved: somewhere near the end.
        assert!(s.contains("repeat #999") || s.contains("repeat #998"));
    }

    #[test]
    fn parse_stage1_happy() {
        let raw = r#"{"problem": "p", "solution_steps": ["s"], "generalizable": true, "scope": "tool", "preconditions": [], "reason": "ok"}"#;
        let a = parse_stage1(raw).unwrap();
        assert!(a.generalizable);
        assert_eq!(a.problem, "p");
    }

    #[test]
    fn parse_stage1_tolerates_markdown_fences() {
        let raw = "```json\n{\"problem\":\"p\",\"generalizable\":false}\n```";
        let a = parse_stage1(raw).unwrap();
        assert!(!a.generalizable);
    }

    #[test]
    fn parse_stage1_tolerates_plain_fences() {
        let raw = "```\n{\"problem\":\"p\",\"generalizable\":false}\n```";
        let a = parse_stage1(raw).unwrap();
        assert!(!a.generalizable);
    }

    #[test]
    fn parse_stage1_rejects_garbage() {
        assert!(parse_stage1("not json").is_err());
    }

    #[test]
    fn validate_accepts_minimal_skill() {
        let md = "---\nname: foo\ndescription: A short description.\n---\n\n# foo\n\nbody.\n";
        let v = validate_skill_md(md).unwrap();
        assert_eq!(v.slug, "foo");
        assert_eq!(v.description, "A short description.");
    }

    #[test]
    fn validate_rejects_missing_frontmatter() {
        let md = "# no frontmatter\nbody";
        assert!(matches!(
            validate_skill_md(md),
            Err(ValidationError::MissingFrontmatter)
        ));
    }

    #[test]
    fn validate_rejects_home_path() {
        let md =
            "---\nname: foo\ndescription: A test.\n---\n\nstep 1: cd /home/alice/repos/x\n";
        assert!(matches!(
            validate_skill_md(md),
            Err(ValidationError::AbsolutePath(_))
        ));
    }

    #[test]
    fn validate_rejects_secret_markers() {
        let md =
            "---\nname: foo\ndescription: A test.\n---\n\nuse token ghp_aaaaaaaaaaaaaaaaaaaa\n";
        assert!(matches!(
            validate_skill_md(md),
            Err(ValidationError::Secret(_))
        ));
    }

    #[test]
    fn bundle_round_trips_through_tar_gz() {
        use flate2::read::GzDecoder;
        use std::io::Read;

        let md = "---\nname: foo\ndescription: A test.\n---\n\nbody.\n";
        let bytes = bundle_skill_md(md).unwrap();
        let gz = GzDecoder::new(&bytes[..]);
        let mut tar = tar::Archive::new(gz);
        let mut found = false;
        for entry in tar.entries().unwrap() {
            let mut entry = entry.unwrap();
            if entry.path().unwrap().to_string_lossy() == "SKILL.md" {
                let mut s = String::new();
                entry.read_to_string(&mut s).unwrap();
                assert_eq!(s, md);
                found = true;
            }
        }
        assert!(found, "SKILL.md not in bundle");
    }

    #[test]
    fn stage1_prompt_includes_transcript() {
        let p = stage1_user_prompt("hello");
        assert!(p.contains("hello"));
        assert!(p.contains("JSON"));
    }

    #[test]
    fn stage2_prompt_embeds_analysis() {
        let a = Stage1Analysis {
            problem: "p".into(),
            solution_steps: vec!["s".into()],
            generalizable: true,
            scope: Some("tool".into()),
            preconditions: vec![],
            reason: None,
        };
        let p = stage2_user_prompt(&a, "hi");
        assert!(p.contains("\"problem\""));
        assert!(p.contains("hi"));
    }
}
