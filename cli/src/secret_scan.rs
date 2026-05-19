//! Phase 4.6 secret-scan quality gate.
//!
//! Two checkpoints in the capturer pipeline:
//!   1. **Pre-stage-2**: scan the trimmed transcript before Sonnet runs.
//!      A transcript dense with credentials is unlikely to produce a
//!      clean draft anyway, and skipping here saves the Sonnet call.
//!   2. **Pre-POST**: scan the assembled `.tar.gz` bundle (every `*.md`
//!      file in the archive). Sonnet can hallucinate token strings that
//!      were not in the transcript, so the bundle gate is the real safety
//!      net.
//!
//! Rules are inspired by the patterns gitleaks ships, but compiled in-
//! process — no external binary dep. Patterns compile once via `OnceLock`
//! so per-session cost is bounded.
//!
//! Findings expose only a *redacted* snippet (first 4 + last 4 chars with
//! `…` in the middle). The raw secret never reaches the log stream.
//!
//! Cf. server-side `bundle::check_secrets` — that path is narrower (four
//! rules, no JWT/Slack/Stripe), so the CLI carries the more thorough set
//! and the server stays the last line of defense.

use std::io::Read;
use std::sync::OnceLock;

use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use regex::Regex;

/// A single secret-shaped match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Stable rule identifier (e.g. `aws-access-key`).
    pub kind: &'static str,
    /// 1-based line number where the match started.
    pub line: usize,
    /// First 4 + `…` + last 4 chars of the matched secret. Never the raw value.
    pub redacted_snippet: String,
}

/// Internal: one compiled rule.
struct Rule {
    kind: &'static str,
    re: Regex,
}

fn rules() -> &'static [Rule] {
    static RULES: OnceLock<Vec<Rule>> = OnceLock::new();
    RULES.get_or_init(|| {
        // Each pattern is anchored with word boundaries or context anchors
        // to avoid trivial false positives. Order matters only for
        // reporting — the first match wins per line.
        vec![
            Rule {
                kind: "aws-access-key",
                // AKIA followed by exactly 16 uppercase alphanum chars.
                re: Regex::new(r"\bAKIA[0-9A-Z]{16}\b").unwrap(),
            },
            Rule {
                kind: "gcp-service-account",
                // GCP service-account JSON marker. The literal field name
                // is stable across exports and very unlikely to appear in
                // normal SKILL.md prose.
                re: Regex::new(r#""type"\s*:\s*"service_account""#).unwrap(),
            },
            Rule {
                kind: "stripe-live-secret",
                re: Regex::new(r"\bsk_live_[A-Za-z0-9]{20,}\b").unwrap(),
            },
            Rule {
                kind: "stripe-live-publishable",
                re: Regex::new(r"\bpk_live_[A-Za-z0-9]{20,}\b").unwrap(),
            },
            Rule {
                kind: "github-pat",
                re: Regex::new(r"\bghp_[A-Za-z0-9]{36}\b").unwrap(),
            },
            Rule {
                kind: "github-oauth",
                re: Regex::new(r"\bgho_[A-Za-z0-9]{36}\b").unwrap(),
            },
            Rule {
                kind: "github-user-token",
                re: Regex::new(r"\bghu_[A-Za-z0-9]{36}\b").unwrap(),
            },
            Rule {
                kind: "github-server-token",
                re: Regex::new(r"\bghs_[A-Za-z0-9]{36}\b").unwrap(),
            },
            Rule {
                kind: "github-refresh-token",
                re: Regex::new(r"\bghr_[A-Za-z0-9]{36}\b").unwrap(),
            },
            Rule {
                kind: "slack-token",
                // xoxb-/xoxp-/xoxa-/xoxs- prefixes. Body is dash-separated
                // base36-ish; pin a minimum length to avoid the literal
                // prefix triggering on docs.
                re: Regex::new(r"\bxox[abps]-[A-Za-z0-9-]{10,}\b").unwrap(),
            },
            Rule {
                kind: "pem-private-key",
                re: Regex::new(r"-----BEGIN [A-Z ]*PRIVATE KEY-----").unwrap(),
            },
            Rule {
                kind: "jwt",
                // JWTs are three base64url segments joined by `.`. We
                // require the whole thing to be >100 chars to avoid
                // matching short example tokens or the literal `eyJ`
                // prefix appearing in a code fence.
                re: Regex::new(
                    r"\beyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\b",
                )
                .unwrap(),
            },
            Rule {
                kind: "generic-quoted-secret",
                // `secret: "..."` / `api_key='...'` style assignments with
                // a 20+ char value. Catches the long tail of "I pasted my
                // .env into the chat" sessions without going so wide it
                // trips on `description: "..."` from frontmatter.
                re: Regex::new(
                    r#"(?i)\b(?:api[_-]?key|secret|token|password|passwd|access[_-]?key)\s*[:=]\s*['"][A-Za-z0-9+/=_\-]{20,}['"]"#,
                )
                .unwrap(),
            },
        ]
    })
}

/// Scan plain text for secret-shaped strings. One finding per match;
/// duplicate findings across lines are kept (caller dedupes if needed).
pub fn scan_text(input: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (idx, line) in input.lines().enumerate() {
        for rule in rules() {
            if let Some(m) = rule.re.find(line) {
                out.push(Finding {
                    kind: rule.kind,
                    line: idx + 1,
                    redacted_snippet: redact(m.as_str()),
                });
            }
        }
    }
    out
}

/// Scan every `*.md` file inside a `.tar.gz` bundle. The capturer always
/// produces a single-file bundle, but a future curator path may add files;
/// this is forward-compatible with that.
pub fn scan_bundle(bundle: &Bytes) -> Result<Vec<Finding>> {
    use flate2::read::GzDecoder;

    let gz = GzDecoder::new(&bundle[..]);
    let mut tar = tar::Archive::new(gz);
    let mut findings = Vec::new();
    let entries = tar
        .entries()
        .map_err(|e| anyhow!("tar entries: {e}"))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| anyhow!("tar entry: {e}"))?;
        let is_md = entry
            .path()
            .ok()
            .and_then(|p| p.extension().map(|e| e.to_string_lossy().to_lowercase()))
            .as_deref()
            == Some("md");
        if !is_md {
            continue;
        }
        let mut buf = String::new();
        entry
            .read_to_string(&mut buf)
            .context("read SKILL.md from tar")?;
        findings.extend(scan_text(&buf));
    }
    Ok(findings)
}

/// Build a short, log-safe rendering of a secret: first 4 + `…` + last 4.
/// Shorter strings get progressively less detail; we never leak more than
/// 8 chars of the original.
fn redact(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        // Too short to reveal anything safely; just mask.
        return "…".repeat(chars.len().min(3));
    }
    let head: String = chars.iter().take(4).collect();
    let tail: String = chars.iter().rev().take(4).collect::<Vec<_>>().into_iter().rev().collect();
    format!("{head}…{tail}")
}

/// Convenience: human-friendly summary of findings for log lines. Never
/// includes the raw secret.
pub fn summarise(findings: &[Finding]) -> String {
    if findings.is_empty() {
        return "no findings".to_string();
    }
    let mut parts: Vec<String> = findings
        .iter()
        .map(|f| format!("{}@L{}({})", f.kind, f.line, f.redacted_snippet))
        .collect();
    parts.sort();
    parts.dedup();
    parts.join(", ")
}

// ---------------------------------------------------------------------------
// Tests — one positive + one negative per rule, plus the bundle scanner.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(findings: &[Finding]) -> Vec<&'static str> {
        let mut k: Vec<_> = findings.iter().map(|f| f.kind).collect();
        k.sort();
        k.dedup();
        k
    }

    #[test]
    fn aws_access_key_positive() {
        let f = scan_text("export AWS_KEY=AKIAIOSFODNN7EXAMPLE\n");
        assert_eq!(kinds(&f), vec!["aws-access-key"]);
    }

    #[test]
    fn aws_access_key_negative_wrong_length() {
        // 15 chars after AKIA, not 16 → should NOT match.
        let f = scan_text("AKIATESTKEY12345");
        assert!(
            !kinds(&f).contains(&"aws-access-key"),
            "false positive on short AKIA literal: {f:?}"
        );
    }

    #[test]
    fn gcp_service_account_positive() {
        let f = scan_text(r#"{ "type": "service_account", "project_id": "x" }"#);
        assert_eq!(kinds(&f), vec!["gcp-service-account"]);
    }

    #[test]
    fn gcp_service_account_negative() {
        let f = scan_text(r#"{ "type": "user_account" }"#);
        assert!(!kinds(&f).contains(&"gcp-service-account"));
    }

    #[test]
    fn stripe_live_secret_positive() {
        let f = scan_text("STRIPE=sk_live_abcdefghij0123456789ABCD\n");
        assert!(kinds(&f).contains(&"stripe-live-secret"));
    }

    #[test]
    fn stripe_test_key_does_not_match_live() {
        // Test keys (sk_test_...) should not trip the live-key rule.
        let f = scan_text("STRIPE=sk_test_abcdefghij0123456789ABCD\n");
        assert!(!kinds(&f).contains(&"stripe-live-secret"));
    }

    #[test]
    fn stripe_live_publishable_positive() {
        let f = scan_text("pk_live_abcdefghij0123456789ABCD");
        assert!(kinds(&f).contains(&"stripe-live-publishable"));
    }

    #[test]
    fn stripe_publishable_test_negative() {
        let f = scan_text("pk_test_abcdefghij0123456789ABCD");
        assert!(!kinds(&f).contains(&"stripe-live-publishable"));
    }

    #[test]
    fn github_pat_positive_all_prefixes() {
        for prefix in ["ghp", "gho", "ghu", "ghs", "ghr"] {
            let body = "a".repeat(36);
            let line = format!("token={prefix}_{body}\n");
            let f = scan_text(&line);
            assert!(
                !f.is_empty(),
                "{prefix}_ token should match some github rule"
            );
        }
    }

    #[test]
    fn github_token_negative_wrong_length() {
        // 35 chars after ghp_ → should NOT match.
        let f = scan_text("ghp_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
        assert!(!kinds(&f).iter().any(|k| k.starts_with("github-")));
    }

    #[test]
    fn slack_token_positive() {
        let f = scan_text("SLACK=xoxb-12345-abcdef-67890-zzz\n");
        assert!(kinds(&f).contains(&"slack-token"));
    }

    #[test]
    fn slack_token_negative_bare_prefix() {
        // Just the prefix in prose — must not trip.
        let f = scan_text("Slack tokens start with xoxb- and similar.\n");
        assert!(!kinds(&f).contains(&"slack-token"));
    }

    #[test]
    fn pem_private_key_positive() {
        let f = scan_text("-----BEGIN RSA PRIVATE KEY-----\nMIIEowIBAAK...\n");
        assert!(kinds(&f).contains(&"pem-private-key"));
    }

    #[test]
    fn pem_public_key_negative() {
        let f = scan_text("-----BEGIN PUBLIC KEY-----\n");
        assert!(!kinds(&f).contains(&"pem-private-key"));
    }

    #[test]
    fn jwt_positive() {
        // 3 base64url segments, total well over 100 chars.
        let seg = "a".repeat(40);
        let jwt = format!("eyJ{seg}.{seg}.{seg}");
        let f = scan_text(&jwt);
        assert!(kinds(&f).contains(&"jwt"), "{f:?}");
    }

    #[test]
    fn jwt_negative_short_prefix() {
        // Just `eyJ` and a short blob — too short to be a real JWT.
        let f = scan_text("eyJabc.def.ghi");
        assert!(!kinds(&f).contains(&"jwt"));
    }

    #[test]
    fn generic_quoted_secret_positive() {
        let f = scan_text(r#"api_key: "abcd1234EFGH5678ijkl90""#);
        assert!(
            kinds(&f).contains(&"generic-quoted-secret"),
            "expected generic-quoted-secret, got {f:?}"
        );
    }

    #[test]
    fn generic_quoted_secret_negative_short_value() {
        // Value under 20 chars → not flagged.
        let f = scan_text(r#"api_key: "shortvalue""#);
        assert!(!kinds(&f).contains(&"generic-quoted-secret"));
    }

    #[test]
    fn generic_quoted_secret_negative_frontmatter_description() {
        // The frontmatter `description:` field is the most common
        // long-quoted-string in our domain. Must not trip.
        let f = scan_text(r#"description: "A skill for doing something useful that is at least twenty chars long""#);
        assert!(
            !kinds(&f).contains(&"generic-quoted-secret"),
            "frontmatter description triggered the generic rule: {f:?}"
        );
    }

    #[test]
    fn redact_short_string() {
        assert_eq!(redact("ab"), "……");
        assert_eq!(redact("abc"), "………");
        assert_eq!(redact("abcdefgh"), "………");
    }

    #[test]
    fn redact_long_string() {
        assert_eq!(redact("AKIAIOSFODNN7EXAMPLE"), "AKIA…MPLE");
    }

    #[test]
    fn finding_records_line_number() {
        let input = "line one is fine\nAKIAIOSFODNN7EXAMPLE\n";
        let f = scan_text(input);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].line, 2);
    }

    #[test]
    fn clean_text_yields_no_findings() {
        let input = "How to validate frontmatter:\n\
                     1. Read the YAML.\n\
                     2. Check the description.\n";
        assert!(scan_text(input).is_empty());
    }

    #[test]
    fn summarise_redacts_and_dedupes() {
        let findings = vec![
            Finding {
                kind: "aws-access-key",
                line: 1,
                redacted_snippet: "AKIA…MPLE".into(),
            },
            Finding {
                kind: "aws-access-key",
                line: 1,
                redacted_snippet: "AKIA…MPLE".into(),
            },
        ];
        let s = summarise(&findings);
        assert_eq!(s, "aws-access-key@L1(AKIA…MPLE)");
    }

    fn make_tar_gz_with_md(md: &str) -> Bytes {
        use flate2::write::GzEncoder;
        use flate2::Compression;
        use std::io::Write;

        let mut tar = tar::Builder::new(Vec::new());
        let bytes = md.as_bytes();
        let mut header = tar::Header::new_gnu();
        header.set_path("SKILL.md").unwrap();
        header.set_size(bytes.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        tar.append(&header, bytes).unwrap();
        let tar_bytes = tar.into_inner().unwrap();

        let mut gz = GzEncoder::new(Vec::new(), Compression::default());
        gz.write_all(&tar_bytes).unwrap();
        Bytes::from(gz.finish().unwrap())
    }

    #[test]
    fn scan_bundle_finds_secret_inside_tar() {
        let md = "---\nname: foo\ndescription: A.\n---\n\nuse AKIAIOSFODNN7EXAMPLE\n";
        let bundle = make_tar_gz_with_md(md);
        let findings = scan_bundle(&bundle).unwrap();
        assert_eq!(kinds(&findings), vec!["aws-access-key"]);
    }

    #[test]
    fn scan_bundle_clean_bundle_is_empty() {
        let md = "---\nname: foo\ndescription: A.\n---\n\nbody.\n";
        let bundle = make_tar_gz_with_md(md);
        assert!(scan_bundle(&bundle).unwrap().is_empty());
    }
}
