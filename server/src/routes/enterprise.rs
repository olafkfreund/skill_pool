//! Enterprise-deployment helpers.
//!
//! `GET /v1/enterprise/managed-settings` — generates a tenant-tailored
//! `managed-settings.json` ready for an IT admin to drop into the Claude
//! Code managed-settings paths on the fleet:
//!
//!   macOS:   /Library/Application Support/ClaudeCode/managed-settings.json
//!   Linux:   /etc/claude-code/managed-settings.json
//!   Windows: C:\Program Files\ClaudeCode\managed-settings.json
//!
//! Admin-only — tokens with `tenant:admin` scope can download. The body
//! contains env-var pins for the skill-pool CLI plus a baseline permissions
//! list that complements (not replaces) whatever Anthropic-side managed
//! settings the IT admin already deploys.

use std::env;

use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};

use crate::auth::AuthedCaller;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub async fn managed_settings(
    State(state): State<AppState>,
    caller: AuthedCaller,
) -> AppResult<Response> {
    if !caller
        .scope
        .split_whitespace()
        .any(|s| s == "tenant:admin" || s == "*")
    {
        return Err(AppError::Forbidden);
    }

    // Public origin defaults to "https://<tenant>.skill-pool.example.com" if
    // SKILL_POOL_PUBLIC_ORIGIN is unset.
    let origin = env::var("SKILL_POOL_PUBLIC_ORIGIN")
        .unwrap_or_else(|_| state.origin_pattern().to_string())
        .trim_end_matches('/')
        .to_string();
    let tenant_origin = origin.replace("{tenant}", &caller.tenant.tenant_slug);

    let payload: Value = json!({
        "$schema": "https://docs.claude.com/_schemas/claude-code-managed-settings.json",
        "_generated_by": "skill-pool-server",
        "_tenant": caller.tenant.tenant_slug,
        "env": {
            "SKILL_POOL_REGISTRY": tenant_origin,
            "SKILL_POOL_TENANT": caller.tenant.tenant_slug,
            // Token file path is read by the skill-pool CLI on every invocation.
            // IT pushes per-machine tokens to /etc/skill-pool/token via MDM.
            "SKILL_POOL_TOKEN_FILE": "/etc/skill-pool/token"
        },
        "additionalDirectories": [
            // Phase 0 install path. skill-pool's CLI symlinks into ~/.claude/skills/
            // already; this entry lets ops push skills system-wide via MDM too.
            "/var/lib/skill-pool/skills"
        ],
        "permissions": {
            "allow": [
                "Bash(skill-pool *)",
                "Bash(skill-pool-server admin *)",
                "Read",
                "Glob",
                "Grep"
            ],
            "deny": [
                // Catch-all destructive guard. IT can extend; this is the baseline.
                "Bash(rm -rf /*)",
                "Bash(rm -rf /)"
            ]
        },
        "_apiKeyHelper_hint":
            "Set apiKeyHelper to a script that prints the Anthropic API key. \
             See docs/enterprise.md — skill-pool ships /usr/local/bin/skill-pool-bootstrap \
             which complements (does not replace) Anthropic's own helper if you use one."
    });

    let body = serde_json::to_string_pretty(&payload).map_err(|e| AppError::Anyhow(e.into()))?;

    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"managed-settings.json\""),
    );
    Ok(resp)
}
