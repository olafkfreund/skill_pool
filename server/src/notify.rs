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

use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use hmac::{Hmac, Mac};
use serde::Serialize;
use sha2::Sha256;
use sqlx::PgPool;
use uuid::Uuid;

use crate::audit;
use crate::email_branding::{self, TransportCache as EmailTransportCache};

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

/// Per-tenant SMTP delivery configuration.
pub struct EmailConfig {
    pub smtp_url: String,
    pub from_addr: String,
    pub to_addr: String,
}

/// Loaded together so the spawned task makes one DB query and then runs
/// the two delivery paths in parallel.
pub struct NotificationConfig {
    pub webhook: Option<WebhookConfig>,
    pub email: Option<EmailConfig>,
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

/// Row tuple returned by the combined webhook + email config query.
/// Aliased to silence `clippy::type_complexity` without losing the
/// inline type info.
type NotificationConfigRow = (
    Option<String>, // webhook url
    Option<String>, // webhook secret
    Option<String>, // smtp url
    Option<String>, // smtp from
    Option<String>, // smtp to
);

impl NotificationConfig {
    /// Single DB round trip that loads both webhook + email configs.
    /// `None` for either side means "skip that delivery channel."
    pub async fn load(db: &PgPool, tenant_id: Uuid) -> sqlx::Result<Self> {
        let row: Option<NotificationConfigRow> = sqlx::query_as(
            "SELECT notifications_webhook_url, notifications_webhook_secret, \
                    notification_smtp_url, notification_smtp_from, notification_smtp_to \
             FROM tenants WHERE id = $1",
        )
        .bind(tenant_id)
        .fetch_optional(db)
        .await?;
        let Some((wh_url, wh_secret, smtp_url, from, to)) = row else {
            return Ok(Self {
                webhook: None,
                email: None,
            });
        };
        let webhook = wh_url.map(|url| WebhookConfig {
            url,
            secret: wh_secret,
        });
        let email = match (smtp_url, from, to) {
            (Some(smtp_url), Some(from_addr), Some(to_addr))
                if !smtp_url.trim().is_empty()
                    && !from_addr.trim().is_empty()
                    && !to_addr.trim().is_empty() =>
            {
                Some(EmailConfig {
                    smtp_url,
                    from_addr,
                    to_addr,
                })
            }
            _ => None,
        };
        Ok(Self { webhook, email })
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
/// from inside the spawned task. Both webhook + email channels fan out
/// from a single tokio task — neither blocks the other.
///
/// `email_transport` is the per-tenant branded SMTP cache. When a tenant
/// has a `tenant_email_branding` row configured, the email branch sends
/// through that cached transport with the branded From / Reply-To /
/// footer. When the row is absent, the legacy global SMTP path
/// (`notification_smtp_*` columns) is used unchanged.
pub fn draft_created(
    db: PgPool,
    email_transport: Arc<EmailTransportCache>,
    ev: DraftCreatedEvent,
) {
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
        let cfg = match NotificationConfig::load(&db, tenant_id).await {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = ?e, "load notification config failed");
                return;
            }
        };
        if cfg.webhook.is_none() && cfg.email.is_none() {
            return; // nothing configured — silent skip
        }

        let text = if let Some(target) = &merge_proposal_slug {
            format!(
                "New draft `{}` in `{}` — looks like an update to existing skill `{}`",
                draft_slug, tenant_slug, target
            )
        } else {
            format!("New draft `{}` ready for review in `{}`", draft_slug, tenant_slug)
        };

        // ---- Webhook fanout ----
        if let Some(wh) = &cfg.webhook {
            let envelope = Envelope {
                text: text.clone(),
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
            match serde_json::to_vec(&envelope) {
                Ok(body) => {
                    let outcome = deliver(wh, &body).await;
                    audit::record_best_effort(
                        &db,
                        audit::Event {
                            tenant_id,
                            actor_user: None,
                            actor_token: None,
                            action: "notification.deliver",
                            target_kind: "webhook",
                            target_id: Some(&draft_id.to_string()),
                            metadata: outcome.to_audit_metadata(&wh.url, envelope.event),
                            ip_addr: None,
                            user_agent: None,
                        },
                    )
                    .await;
                }
                Err(e) => {
                    tracing::error!(error = ?e, "serialise notification envelope")
                }
            }
        }

        // ---- Email fanout ----
        if let Some(em) = &cfg.email {
            let subject = format!("[skill-pool] New draft \"{}\" in {}", draft_slug, tenant_slug);
            let body = build_email_body(
                &tenant_slug,
                &draft_slug,
                &description,
                &origin,
                merge_proposal_slug.as_deref(),
            );

            // Branded path: tenant has per-tenant SMTP + From override.
            // We still respect `cfg.email.to_addr` from the legacy table
            // as the recipient — that's where curators expect to be
            // notified. Only the transport + From line are branded.
            let branded_row = match email_branding::load_row(&db, tenant_id).await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!(error = ?e, "load tenant_email_branding failed; falling back to global SMTP");
                    None
                }
            };

            let (metadata, _channel) = if let Some(row) = branded_row {
                let outcome = email_branding::send_branded(
                    email_transport.as_ref(),
                    &row,
                    &em.to_addr,
                    &subject,
                    &body,
                )
                .await;
                (
                    outcome.to_audit_metadata(&em.to_addr, &row.from_addr),
                    "branded",
                )
            } else {
                let outcome = send_email(em, &subject, &body).await;
                (outcome.to_email_audit_metadata(&em.to_addr), "global")
            };

            audit::record_best_effort(
                &db,
                audit::Event {
                    tenant_id,
                    actor_user: None,
                    actor_token: None,
                    action: "notification.deliver",
                    target_kind: "email",
                    target_id: Some(&draft_id.to_string()),
                    metadata,
                    ip_addr: None,
                    user_agent: None,
                },
            )
            .await;
        }
    });
}

/// Plain-text email body. Mirrors the webhook envelope's `text` field
/// but with full structured info so a curator reading the email has
/// everything they need without opening the portal.
pub(crate) fn build_email_body(
    tenant_slug: &str,
    draft_slug: &str,
    description: &str,
    origin: &str,
    merge_proposal_slug: Option<&str>,
) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "A new skill draft is ready for review in `{tenant_slug}`.\n\n",
    ));
    s.push_str(&format!("Slug:        {draft_slug}\n"));
    s.push_str(&format!("Description: {description}\n"));
    s.push_str(&format!("Origin:      {origin}\n"));
    if let Some(target) = merge_proposal_slug {
        s.push_str(&format!(
            "Merge candidate: looks similar to existing skill `{target}`.\n",
        ));
    }
    s.push_str("\nReview in the portal: /drafts\n");
    s.push_str("\n--\nskill-pool (do not reply)\n");
    s
}

#[derive(Debug)]
enum EmailOutcome {
    Success,
    Failed(String),
}

impl EmailOutcome {
    fn to_email_audit_metadata(&self, to_addr: &str) -> serde_json::Value {
        match self {
            EmailOutcome::Success => serde_json::json!({
                "result": "success",
                "to": to_addr,
            }),
            EmailOutcome::Failed(msg) => serde_json::json!({
                "result": "failed",
                "to": to_addr,
                "error": msg,
            }),
        }
    }
}

async fn send_email(cfg: &EmailConfig, subject: &str, body: &str) -> EmailOutcome {
    use lettre::message::Mailbox;
    use lettre::transport::smtp::AsyncSmtpTransport;
    use lettre::{AsyncTransport, Message, Tokio1Executor};

    // Parse from + to as `Name <addr@host>` or bare `addr@host`.
    let from: Mailbox = match cfg.from_addr.parse() {
        Ok(m) => m,
        Err(e) => return EmailOutcome::Failed(format!("invalid from address: {e}")),
    };
    let to: Mailbox = match cfg.to_addr.parse() {
        Ok(m) => m,
        Err(e) => return EmailOutcome::Failed(format!("invalid to address: {e}")),
    };

    let msg = match Message::builder()
        .from(from)
        .to(to)
        .subject(subject)
        .body(body.to_string())
    {
        Ok(m) => m,
        Err(e) => return EmailOutcome::Failed(format!("build message: {e}")),
    };

    let mailer: AsyncSmtpTransport<Tokio1Executor> =
        match AsyncSmtpTransport::<Tokio1Executor>::from_url(&cfg.smtp_url) {
            Ok(t) => t.build(),
            Err(e) => return EmailOutcome::Failed(format!("parse smtp url: {e}")),
        };

    match mailer.send(msg).await {
        Ok(_) => EmailOutcome::Success,
        Err(e) => EmailOutcome::Failed(e.to_string()),
    }
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
