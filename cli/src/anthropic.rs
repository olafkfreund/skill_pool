//! Minimal Anthropic Messages API client.
//!
//! Built directly on `reqwest` rather than a heavier SDK because we only
//! need two operations (one chat call per stage) and want zero extra deps.
//!
//! API key sourcing: `ANTHROPIC_API_KEY` env var, matching Claude Code's
//! own convention.

use anyhow::{anyhow, Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, CONTENT_TYPE};
use serde::{Deserialize, Serialize};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: u64 = 60;

#[derive(Debug, Clone)]
pub struct AnthropicClient {
    http: reqwest::Client,
    api_url: String,
    api_key: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateMessageRequest<'a> {
    pub model: &'a str,
    pub max_tokens: u32,
    pub system: &'a str,
    pub messages: Vec<Message<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message<'a> {
    pub role: &'a str,
    pub content: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMessageResponse {
    pub content: Vec<ContentBlock>,
    /// Surfaced by the API but not currently consumed by the orchestrator;
    /// kept on the type so debug logs and future retry logic can see it.
    #[allow(dead_code)]
    pub stop_reason: Option<String>,
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ContentBlock {
    #[serde(rename = "text")]
    Text { text: String },
    /// Anthropic may return other block types in future; we ignore them
    /// rather than fail to parse.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}

impl AnthropicClient {
    /// Build from `ANTHROPIC_API_KEY` env var. Returns an error if the key
    /// is missing — the orchestrator surfaces this as a friendly "set the
    /// env var" message rather than crashing the cron job.
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("ANTHROPIC_API_KEY")
            .context("ANTHROPIC_API_KEY is not set — required for the capturer")?;
        Self::with_url(api_key, API_URL.to_string())
    }

    /// For tests: target a stub server. Production paths go through `from_env`.
    pub fn with_url(api_key: String, api_url: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_TIMEOUT_SECS))
            .user_agent(concat!("skill-pool-capturer/", env!("CARGO_PKG_VERSION")))
            .build()?;
        Ok(Self {
            http,
            api_url,
            api_key,
        })
    }

    /// Call Messages API once. Returns the concatenated text of all `text`
    /// content blocks. Errors surface the HTTP body for triage.
    pub async fn create_message<'a>(&self, req: CreateMessageRequest<'a>) -> Result<String> {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&self.api_key).context("invalid api key bytes")?,
        );
        headers.insert(
            "anthropic-version",
            HeaderValue::from_static(API_VERSION),
        );
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));

        let resp = self
            .http
            .post(&self.api_url)
            .headers(headers)
            .json(&req)
            .send()
            .await
            .context("send anthropic request")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("anthropic {status}: {body}"));
        }

        let body: CreateMessageResponse = resp.json().await.context("parse anthropic response")?;
        let mut text = String::new();
        for block in body.content {
            if let ContentBlock::Text { text: t } = block {
                text.push_str(&t);
            }
        }
        if text.is_empty() {
            return Err(anyhow!("anthropic returned no text content"));
        }
        if let Some(usage) = body.usage {
            tracing::debug!(
                input = usage.input_tokens,
                output = usage.output_tokens,
                "anthropic call"
            );
        }
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_text_content_blocks() {
        let raw = r#"{
            "content": [
                {"type": "text", "text": "hello"},
                {"type": "text", "text": " world"}
            ],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 10, "output_tokens": 2}
        }"#;
        let r: CreateMessageResponse = serde_json::from_str(raw).unwrap();
        let combined: String = r
            .content
            .into_iter()
            .filter_map(|b| match b {
                ContentBlock::Text { text } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(combined, "hello world");
    }

    #[test]
    fn ignores_unknown_content_block_types() {
        let raw = r#"{
            "content": [
                {"type": "text", "text": "keep"},
                {"type": "tool_use", "id": "x", "name": "n", "input": {}}
            ]
        }"#;
        let r: CreateMessageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.content.len(), 2);
        // First is Text("keep"); second is Other.
        assert!(matches!(r.content[1], ContentBlock::Other));
    }
}
