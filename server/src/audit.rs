//! Audit log writes. Every mutating endpoint MUST call `record` exactly once
//! on success. Failures to record are logged but do not fail the request —
//! the alternative is leaving the user uncertain whether their write
//! actually applied. The persistence path is at-least-once via the DB; the
//! audit path is best-effort plus monitor-alerting (see `docs/audit.md`).

use anyhow::Result;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

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

pub async fn record(db: &PgPool, ev: Event<'_>) -> Result<()> {
    // ip_addr column is INET; bind as text and cast explicitly so NULL/Some(String) both work.
    sqlx::query(
        "INSERT INTO audit_events \
         (tenant_id, actor_user, actor_token, action, target_kind, target_id, metadata, ip_addr, user_agent) \
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8::text::inet, $9)",
    )
    .bind(ev.tenant_id)
    .bind(ev.actor_user)
    .bind(ev.actor_token)
    .bind(ev.action)
    .bind(ev.target_kind)
    .bind(ev.target_id)
    .bind(ev.metadata)
    .bind(ev.ip_addr.map(|ip| ip.to_string()))
    .bind(ev.user_agent)
    .execute(db)
    .await?;
    Ok(())
}

/// Convenience wrapper that swallows errors after logging — used by
/// handlers where the response has already been committed.
pub async fn record_best_effort(db: &PgPool, ev: Event<'_>) {
    if let Err(e) = record(db, ev).await {
        tracing::error!(error = ?e, "audit write failed");
    }
}
