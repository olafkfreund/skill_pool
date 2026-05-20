//! Audit log writes. Every mutating endpoint MUST call `record` exactly once
//! on success. Failures to record are logged but do not fail the request —
//! the alternative is leaving the user uncertain whether their write
//! actually applied. The persistence path is at-least-once via the DB; the
//! audit path is best-effort plus monitor-alerting (see `docs/audit.md`).
//!
//! SIEM fan-out: when the tenant has `tenant_audit_siem_url` configured,
//! every successful DB insert is mirrored to that receiver as a JSON POST.
//! Splunk HEC and Datadog Logs both accept "POST JSON with bearer auth",
//! so one URL+token pair covers the common cases. Delivery is spawned on
//! a detached tokio task with a short timeout, and outcomes are logged
//! via `tracing` only — we deliberately do NOT re-audit them, because
//! that would feedback-loop a single source event into infinite SIEM POSTs.

use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

const SIEM_DELIVERY_TIMEOUT_SECS: u64 = 5;
const USER_AGENT: &str = concat!("skill-pool-server/", env!("CARGO_PKG_VERSION"));

pub struct Event<'a> {
    pub tenant_id: Uuid,
    pub actor_user: Option<Uuid>,
    pub actor_token: Option<Uuid>,
    pub action: &'a str,
    pub target_kind: &'a str,
    pub target_id: Option<&'a str>,
    pub metadata: Value,
    pub ip_addr: Option<std::net::IpAddr>,
    pub user_agent: Option<&'a str>,
}

/// Payload shipped to the SIEM receiver. Mirrors the `audit_events` row
/// schema 1:1 so a downstream pipeline can index on the same fields.
#[derive(Serialize)]
struct SiemPayload<'a> {
    id: i64,
    tenant_id: Uuid,
    actor_user: Option<Uuid>,
    actor_token: Option<Uuid>,
    action: &'a str,
    target_kind: &'a str,
    target_id: Option<&'a str>,
    metadata: &'a Value,
    ip_addr: Option<String>,
    user_agent: Option<&'a str>,
    ts: DateTime<Utc>,
}

/// Module-shared `reqwest::Client` so we don't allocate a new client per
/// delivery. Mirrors the curator notification client (see `notify.rs`).
fn siem_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(SIEM_DELIVERY_TIMEOUT_SECS))
            .user_agent(USER_AGENT)
            .build()
            .expect("build SIEM HTTP client")
    })
}

pub async fn record(db: &PgPool, ev: Event<'_>) -> Result<()> {
    // ip_addr column is INET; bind as text and cast explicitly so NULL/Some(String) both work.
    // RETURNING gives us the row id + canonical ts to ship to the SIEM —
    // saves a second round trip when fan-out is configured.
    // JUSTIFIED runtime-checked: `ip_addr` binds to an `INET` column via a
    // `$8::text::inet` cast so we can pass a nullable `Option<String>` without
    // requiring the `sqlx-postgres` inet codec. The `query!` macro cannot
    // express `$n::text::inet` as a compile-time literal type override for an
    // `Option<String>` argument, so we keep this as `query_as`.
    let (id, ts): (i64, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO audit_events \
         (tenant_id, actor_user, actor_token, action, target_kind, target_id, metadata, ip_addr, user_agent) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::text::inet, $9) \
         RETURNING id, ts",
    )
    .bind(ev.tenant_id)
    .bind(ev.actor_user)
    .bind(ev.actor_token)
    .bind(ev.action)
    .bind(ev.target_kind)
    .bind(ev.target_id)
    .bind(&ev.metadata)
    .bind(ev.ip_addr.map(|ip| ip.to_string()))
    .bind(ev.user_agent)
    .fetch_one(db)
    .await?;

    siem_fanout(db, id, ts, &ev);
    Ok(())
}

/// Convenience wrapper that swallows errors after logging — used by
/// handlers where the response has already been committed.
pub async fn record_best_effort(db: &PgPool, ev: Event<'_>) {
    if let Err(e) = record(db, ev).await {
        tracing::error!(error = ?e, "audit write failed");
    }
}

/// Spawn a detached delivery to the tenant's SIEM URL. Cheap when no SIEM
/// is configured (one indexed SELECT then return). Owns its own copy of
/// the payload so the spawned task is `'static`.
fn siem_fanout(db: &PgPool, id: i64, ts: DateTime<Utc>, ev: &Event<'_>) {
    // Build the owned payload up-front so the spawned future doesn't
    // hold any borrows from the caller's stack.
    let payload = OwnedSiemPayload {
        id,
        tenant_id: ev.tenant_id,
        actor_user: ev.actor_user,
        actor_token: ev.actor_token,
        action: ev.action.to_string(),
        target_kind: ev.target_kind.to_string(),
        target_id: ev.target_id.map(|s| s.to_string()),
        metadata: ev.metadata.clone(),
        ip_addr: ev.ip_addr.map(|ip| ip.to_string()),
        user_agent: ev.user_agent.map(|s| s.to_string()),
        ts,
    };
    let db = db.clone();
    tokio::spawn(async move {
        deliver_siem(db, payload).await;
    });
}

struct OwnedSiemPayload {
    id: i64,
    tenant_id: Uuid,
    actor_user: Option<Uuid>,
    actor_token: Option<Uuid>,
    action: String,
    target_kind: String,
    target_id: Option<String>,
    metadata: Value,
    ip_addr: Option<String>,
    user_agent: Option<String>,
    ts: DateTime<Utc>,
}

async fn deliver_siem(db: PgPool, p: OwnedSiemPayload) {
    let cfg = match SiemConfig::load(&db, p.tenant_id).await {
        Ok(Some(c)) => c,
        Ok(None) => return, // no URL configured — silent skip (common path)
        Err(e) => {
            tracing::warn!(error = ?e, tenant_id = %p.tenant_id, "load SIEM config failed");
            return;
        }
    };

    let wire = SiemPayload {
        id: p.id,
        tenant_id: p.tenant_id,
        actor_user: p.actor_user,
        actor_token: p.actor_token,
        action: &p.action,
        target_kind: &p.target_kind,
        target_id: p.target_id.as_deref(),
        metadata: &p.metadata,
        ip_addr: p.ip_addr.clone(),
        user_agent: p.user_agent.as_deref(),
        ts: p.ts,
    };
    let body = match serde_json::to_vec(&wire) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = ?e, "serialise SIEM payload");
            return;
        }
    };

    let mut req = siem_http_client()
        .post(&cfg.url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(body);
    if let Some(token) = &cfg.token {
        req = req.bearer_auth(token);
    }

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                tracing::warn!(
                    %status,
                    url = %cfg.url,
                    tenant_id = %p.tenant_id,
                    "SIEM receiver returned non-2xx"
                );
            }
        }
        Err(e) => {
            tracing::warn!(
                error = ?e,
                url = %cfg.url,
                tenant_id = %p.tenant_id,
                "SIEM POST failed",
            );
        }
    }
}

/// Per-tenant SIEM destination loaded for a single delivery.
pub struct SiemConfig {
    pub url: String,
    pub token: Option<String>,
}

impl SiemConfig {
    /// Returns `None` when the tenant has no URL configured — fan-out is
    /// silently skipped for that case (the common path).
    pub async fn load(db: &PgPool, tenant_id: Uuid) -> sqlx::Result<Option<Self>> {
        let row = sqlx::query!(
            "SELECT tenant_audit_siem_url, tenant_audit_siem_token \
             FROM tenants WHERE id = $1",
            tenant_id,
        )
        .fetch_optional(db)
        .await?;
        Ok(row.and_then(|r| r.tenant_audit_siem_url.map(|u| Self { url: u, token: r.tenant_audit_siem_token })))
    }
}
