//! Curator notifications (Phase 5).
//!
//! Fire-and-forget webhook delivery on draft events. The HTTP response to
//! the original API call returns immediately; delivery runs on a detached
//! tokio task with a hard timeout. Outcomes — success, transport error,
//! HTTP 4xx/5xx — are written to `audit_events` so operators can see
//! what's happening without hunting through logs.
//!
//! Payload is generic JSON with a top-level `text` field (Slack/Discord
//! incoming-webhook compatible) plus structured fields for tooling.
//!
//! Signing: when the tenant configured a webhook secret, the body bytes
//! are signed with HMAC-SHA256 and the hex digest is shipped in
//! `X-Skill-Pool-Signature: sha256=<hex>`. Matches GitHub/Stripe convention.

use std::sync::OnceLock;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;

const DELIVERY_TIMEOUT_SECS: u64 = 5;
const SIGNATURE_HEADER: &str = "X-Skill-Pool-Signature";
const USER_AGENT: &str = concat!("skill-pool-server/", env!("CARGO_PKG_VERSION"));

/// Module-shared `reqwest::Client` so we don't allocate a new client per
/// delivery. Connection pooling matters when many tenants are firing
/// concurrent webhooks.
fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(DELIVERY_TIMEOUT_SECS))
            .user_agent(USER_AGENT)
            .build()
            .expect("build notification HTTP client")
    })
}

#[derive(Serialize)]
struct Envelope<'a> {
    /// Human-readable summary. Slack/Discord render this as the message
    /// text when no other fields are interpreted.
    text: String,
    /// Event name. Currently only `draft.created`; `draft.merge_proposed`
    /// and `skill.published` are obvious next slots.
    event: &'a str,
    tenant: TenantField<'a>,
    draft: Option<DraftField<'a>>,
}

#[derive(Serialize)]
struct TenantField<'a> {
    slug: &'a str,
}

#[derive(Serialize)]
struct DraftField<'a> {
    id: Uuid,
    slug: &'a str,
    description: &'a str,
    origin: &'a str,
    /// Set when embedding dedup attached a merge proposal at create time.
    merge_proposal_slug: Option<&'a str>,
}

/// Per-tenant webhook configuration loaded for a single delivery.
pub struct WebhookConfig {
    pub url: String,
    pub secret: Option<String>,
}

impl WebhookConfig {
    /// Loads from `tenants.notifications_webhook_url` /
    /// `notifications_webhook_secret`. Returns `None` when the tenant has
    /// no URL configured — delivery is silently skipped.
    pub async fn load(db: &PgPool, tenant_id: Uuid) -> sqlx::Result<Option<Self>> {
        let row: Option<(Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT notifications_webhook_url, notifications_webhook_secret \
             FROM tenants WHERE id = $1",
        )
        .bind(tenant_id)
        .fetch_optional(db)
        .await?;
        Ok(row.and_then(|(url, secret)| url.map(|u| Self { url: u, secret })))
    }
}

/// Compute `sha256=<hex>` over the body using the configured secret.
pub(crate) fn sign_body(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA256 accepts any key length");
    mac.update(body);
    format!("sha256={}", hex::encode(mac.finalize().into_bytes()))
}

/// Inputs the spawned delivery task needs. Bundled into a single struct
/// so the public entry point doesn't grow a wider argument list every
/// time we add a contextual field.
pub struct DraftCreatedEvent {
    pub tenant_id: Uuid,
    pub tenant_slug: String,
    pub draft_id: Uuid,
    pub draft_slug: String,
    pub description: String,
    pub origin: String,
    pub merge_proposal_slug: Option<String>,
}

/// Fire-and-forget delivery. Returns immediately; outcome is audit-logged
/// from inside the spawned task.
pub fn draft_created(db: PgPool, ev: DraftCreatedEvent) {
    let DraftCreatedEvent {
        tenant_id,
        tenant_slug,
        draft_id,
        draft_slug,
        description,
        origin,
        merge_proposal_slug,
    } = ev;
    tokio::spawn(async move {
        let cfg = match WebhookConfig::load(&db, tenant_id).await {
            Ok(Some(c)) => c,
            Ok(None) => return, // no webhook configured
            Err(e) => {
                tracing::warn!(error = ?e, "load webhook config failed");
                return;
            }
        };

        let text = if let Some(target) = &merge_proposal_slug {
            format!(
                "New draft `{}` in `{}` — looks like an update to existing skill `{}`",
                draft_slug, tenant_slug, target
            )
        } else {
            format!("New draft `{}` ready for review in `{}`", draft_slug, tenant_slug)
        };

        let envelope = Envelope {
            text,
            event: "draft.created",
            tenant: TenantField { slug: &tenant_slug },
            draft: Some(DraftField {
                id: draft_id,
                slug: &draft_slug,
                description: &description,
                origin: &origin,
                merge_proposal_slug: merge_proposal_slug.as_deref(),
            }),
        };

        let body = match serde_json::to_vec(&envelope) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(error = ?e, "serialise notification envelope");
                return;
            }
        };

        let outcome = deliver(&cfg, &body).await;
        audit::record_best_effort(
            &db,
            audit::Event {
                tenant_id,
                actor_user: None,
                actor_token: None,
                action: "notification.deliver",
                target_kind: "webhook",
                target_id: Some(&draft_id.to_string()),
                metadata: outcome.to_audit_metadata(&cfg.url, envelope.event),
                ip_addr: None,
                user_agent: None,
            },
        )
        .await;
    });
}

/// One HTTP attempt + a single retry on transient error. Returns a
/// structured outcome that becomes the audit metadata.
async fn deliver(cfg: &WebhookConfig, body: &[u8]) -> DeliveryOutcome {
    let mut last_err = None;
    for attempt in 1..=2 {
        match deliver_once(cfg, body).await {
            Ok(status) if status < 400 => {
                return DeliveryOutcome::Success { status, attempts: attempt };
            }
            Ok(status) => {
                // 4xx is permanent — don't retry.
                if (400..500).contains(&status) {
                    return DeliveryOutcome::HttpError { status, attempts: attempt };
                }
                last_err = Some(format!("HTTP {status}"));
            }
            Err(e) => {
                last_err = Some(e.to_string());
            }
        }
    }
    DeliveryOutcome::Failed {
        attempts: 2,
        last_error: last_err.unwrap_or_else(|| "unknown".to_string()),
    }
}

async fn deliver_once(cfg: &WebhookConfig, body: &[u8]) -> reqwest::Result<u16> {
    let mut req = http_client()
        .post(&cfg.url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body.to_vec());
    if let Some(secret) = &cfg.secret {
        req = req.header(SIGNATURE_HEADER, sign_body(secret, body));
    }
    let resp = req.send().await?;
    Ok(resp.status().as_u16())
}

#[derive(Debug)]
enum DeliveryOutcome {
    Success { status: u16, attempts: u32 },
    HttpError { status: u16, attempts: u32 },
    Failed { attempts: u32, last_error: String },
}

impl DeliveryOutcome {
    fn to_audit_metadata(&self, url: &str, event: &str) -> serde_json::Value {
        match self {
            DeliveryOutcome::Success { status, attempts } => serde_json::json!({
                "result": "success",
                "status": status,
                "attempts": attempts,
                "url": url,
                "event": event,
            }),
            DeliveryOutcome::HttpError { status, attempts } => serde_json::json!({
                "result": "http_error",
                "status": status,
                "attempts": attempts,
                "url": url,
                "event": event,
            }),
            DeliveryOutcome::Failed { attempts, last_error } => serde_json::json!({
                "result": "failed",
                "attempts": attempts,
                "last_error": last_error,
                "url": url,
                "event": event,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_is_deterministic_and_hex() {
        let body = b"hello";
        let sig = sign_body("topsecret", body);
        assert!(sig.starts_with("sha256="));
        // 32 bytes -> 64 hex chars after the `sha256=` prefix.
        assert_eq!(sig.len(), 7 + 64);
        assert_eq!(sig, sign_body("topsecret", body));
    }

    #[test]
    fn signature_changes_with_body() {
        assert_ne!(
            sign_body("k", b"a"),
            sign_body("k", b"b"),
        );
    }

    #[test]
    fn signature_changes_with_secret() {
        assert_ne!(
            sign_body("k1", b"body"),
            sign_body("k2", b"body"),
        );
    }
}
