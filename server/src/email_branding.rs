//! Per-tenant branded transactional email (#9).
//!
//! This module owns three things:
//!
//!   1. **At-rest encryption** of per-tenant SMTP passwords. Plaintext
//!      never lands in Postgres; we round-trip via AES-256-GCM with a
//!      key sourced from `SKILL_POOL_EMAIL_SECRET_KEY` (32 bytes, hex).
//!      When that env is unset we degrade to base64-encoded plaintext
//!      with a loud warning — fine for dev / single-tenant self-host,
//!      not for production. The header byte of the stored blob carries
//!      the format version so we can tell encrypted from base64 fallback
//!      without a schema flag.
//!
//!   2. **A lazy per-tenant SMTP transport cache** mirroring
//!      `AppState::storage_for`. Building a lettre `AsyncSmtpTransport`
//!      is comparatively cheap, but we still don't want to do it on
//!      every send when an enterprise tenant fires off a notification
//!      digest hourly. Keyed by `tenant_id`; never evicts (enterprise
//!      tenants reuse the same SMTP config for the life of the
//!      process).
//!
//!   3. **The send-with-branding entry point** that the existing
//!      `notify::send_email` path defers to when a tenant has a row in
//!      `tenant_email_branding`. The fallback (no row) keeps using the
//!      legacy `notify::send_email` path against the `tenants.notification_smtp_*`
//!      columns, so existing operators see zero behavioural change.

use std::collections::HashMap;
use std::sync::Arc;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};
use lettre::message::Mailbox;
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::{AsyncTransport, Message, Tokio1Executor};
use rand::RngCore;
use sqlx::PgPool;
use tokio::sync::Mutex;
use uuid::Uuid;

/// Env var holding the 32-byte AES-256-GCM key in hex (64 hex chars).
pub const ENCRYPTION_KEY_ENV: &str = "SKILL_POOL_EMAIL_SECRET_KEY";

/// Magic byte at the start of `smtp_password_enc` identifying the
/// encryption format. Lets us bump the scheme later without a migration.
const FMT_AES256_GCM_V1: u8 = 0x01;
/// Fallback marker when no key is configured. Plaintext is base64-d so
/// the column stays text-safe at the BYTEA layer.
const FMT_PLAINTEXT_B64_V0: u8 = 0x00;

/// Row shape returned by `load_row` for use by both the send path and
/// the admin endpoints (which strip the password before returning).
pub struct BrandingRow {
    pub tenant_id: Uuid,
    pub from_addr: String,
    pub from_name: Option<String>,
    pub reply_to: Option<String>,
    pub smtp_url: String,
    pub smtp_password_enc: Vec<u8>,
    pub footer_html: Option<String>,
}

/// Encrypt a password for storage. Pulls the key from the env at call
/// time so tests / one-shot processes can mutate it. Returns ciphertext
/// prefixed with the format byte + 12-byte nonce.
pub fn encrypt_password(plaintext: &str) -> Vec<u8> {
    match load_key() {
        Some(key) => {
            let cipher = Aes256Gcm::new(&key.into());
            let mut nonce_bytes = [0u8; 12];
            rand::thread_rng().fill_bytes(&mut nonce_bytes);
            let nonce = Nonce::from_slice(&nonce_bytes);
            match cipher.encrypt(nonce, plaintext.as_bytes()) {
                Ok(ct) => {
                    let mut out = Vec::with_capacity(1 + 12 + ct.len());
                    out.push(FMT_AES256_GCM_V1);
                    out.extend_from_slice(&nonce_bytes);
                    out.extend_from_slice(&ct);
                    out
                }
                Err(e) => {
                    // AES-GCM encryption is infallible for valid keys
                    // and reasonable plaintext sizes; this branch is a
                    // defensive log + fallback rather than a panic.
                    tracing::error!(error = ?e, "email branding: AES-GCM encrypt failed; falling back to base64");
                    plaintext_fallback(plaintext)
                }
            }
        }
        None => {
            tracing::warn!(
                env = ENCRYPTION_KEY_ENV,
                "email branding: {} unset — storing SMTP password as base64 (NOT encrypted). \
                 Set this env in production deployments.",
                ENCRYPTION_KEY_ENV
            );
            plaintext_fallback(plaintext)
        }
    }
}

fn plaintext_fallback(plaintext: &str) -> Vec<u8> {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(plaintext.as_bytes());
    let mut out = Vec::with_capacity(1 + b64.len());
    out.push(FMT_PLAINTEXT_B64_V0);
    out.extend_from_slice(b64.as_bytes());
    out
}

/// Decrypt a blob produced by `encrypt_password`. Returns the plaintext
/// password. Format byte selects the codec; mismatched formats produce
/// a descriptive error rather than silently misdecoding.
pub fn decrypt_password(blob: &[u8]) -> Result<String> {
    let (&tag, rest) = blob
        .split_first()
        .ok_or_else(|| anyhow!("encrypted password blob is empty"))?;
    match tag {
        FMT_AES256_GCM_V1 => {
            let key = load_key().ok_or_else(|| {
                anyhow!(
                    "{} unset but stored blob is AES-GCM ciphertext; cannot decrypt",
                    ENCRYPTION_KEY_ENV
                )
            })?;
            if rest.len() < 12 {
                return Err(anyhow!("AES-GCM blob missing nonce"));
            }
            let (nonce_bytes, ct) = rest.split_at(12);
            let cipher = Aes256Gcm::new(&key.into());
            let nonce = Nonce::from_slice(nonce_bytes);
            let pt = cipher
                .decrypt(nonce, ct)
                .map_err(|e| anyhow!("AES-GCM decrypt: {e}"))?;
            String::from_utf8(pt).map_err(|e| anyhow!("decrypted bytes not UTF-8: {e}"))
        }
        FMT_PLAINTEXT_B64_V0 => {
            use base64::Engine;
            let pt = base64::engine::general_purpose::STANDARD
                .decode(rest)
                .map_err(|e| anyhow!("base64 decode: {e}"))?;
            String::from_utf8(pt).map_err(|e| anyhow!("decoded bytes not UTF-8: {e}"))
        }
        other => Err(anyhow!(
            "unknown email-branding password format byte: {other:#04x}"
        )),
    }
}

/// Read the 32-byte key from the env. Returns `None` when unset,
/// `Some(_)` only when a hex-decoded 32 bytes is available. Other
/// failure shapes (malformed hex, wrong length) log a warning and
/// behave as if the env were unset.
fn load_key() -> Option<[u8; 32]> {
    let raw = std::env::var(ENCRYPTION_KEY_ENV).ok()?;
    let bytes = match hex::decode(raw.trim()) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "{} is not valid hex; treating as unset", ENCRYPTION_KEY_ENV);
            return None;
        }
    };
    if bytes.len() != 32 {
        tracing::warn!(
            len = bytes.len(),
            "{} must be exactly 32 bytes (64 hex chars); treating as unset",
            ENCRYPTION_KEY_ENV
        );
        return None;
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Some(out)
}

/// Fetch the branding row for a tenant. `None` ⇒ no per-tenant
/// branding configured; caller falls back to global SMTP.
pub async fn load_row(db: &PgPool, tenant_id: Uuid) -> sqlx::Result<Option<BrandingRow>> {
    let row = sqlx::query!(
        "SELECT tenant_id, from_addr, from_name, reply_to, smtp_url, smtp_password_enc, footer_html \
         FROM tenant_email_branding WHERE tenant_id = $1",
        tenant_id,
    )
    .fetch_optional(db)
    .await?;
    Ok(row.map(|r| BrandingRow {
        tenant_id: r.tenant_id,
        from_addr: r.from_addr,
        from_name: r.from_name,
        reply_to: r.reply_to,
        smtp_url: r.smtp_url,
        smtp_password_enc: r.smtp_password_enc,
        footer_html: r.footer_html,
    }))
}

/// Per-tenant SMTP transport cache. Built on first send and re-used.
/// Wrap as `Arc<TransportCache>` and hang off `AppState`.
#[derive(Default)]
pub struct TransportCache {
    inner: Mutex<HashMap<Uuid, Arc<AsyncSmtpTransport<Tokio1Executor>>>>,
}

impl TransportCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns a cached transport for the tenant, or builds one from the
    /// branding row. The cache key is the tenant id; if you mutate the
    /// row (PUT /v1/tenant/email-branding) call `invalidate` so the next
    /// send rebuilds.
    pub async fn get_or_build(
        &self,
        row: &BrandingRow,
    ) -> Result<Arc<AsyncSmtpTransport<Tokio1Executor>>> {
        // Hot path.
        if let Some(t) = self.inner.lock().await.get(&row.tenant_id).cloned() {
            return Ok(t);
        }

        // Cold path: build the transport with the decrypted password
        // re-injected into the URL.
        let password = decrypt_password(&row.smtp_password_enc)?;
        let url_with_pass = inject_password(&row.smtp_url, &password)?;
        let transport: AsyncSmtpTransport<Tokio1Executor> =
            AsyncSmtpTransport::<Tokio1Executor>::from_url(&url_with_pass)
                .map_err(|e| anyhow!("parse smtp url: {e}"))?
                .build();
        let arc = Arc::new(transport);

        let mut cache = self.inner.lock().await;
        // Double-check on the cold->hot transition.
        if let Some(existing) = cache.get(&row.tenant_id) {
            return Ok(existing.clone());
        }
        cache.insert(row.tenant_id, arc.clone());
        Ok(arc)
    }

    pub async fn invalidate(&self, tenant_id: Uuid) {
        self.inner.lock().await.remove(&tenant_id);
    }
}

/// Splice the decrypted password into the SMTP URL. Stored URL is
/// `smtps://user@host:port` (no password); we rebuild `smtps://user:pw@host:port`
/// before handing to lettre. Avoids round-tripping the plaintext
/// through column values.
fn inject_password(stored: &str, password: &str) -> Result<String> {
    let mut url = url::Url::parse(stored).map_err(|e| anyhow!("parse smtp url: {e}"))?;
    url.set_password(Some(password))
        .map_err(|_| anyhow!("smtp url does not support userinfo"))?;
    Ok(url.into())
}

/// Outcome of a branded send. Mirrors `notify::EmailOutcome` shape so
/// audit metadata stays consistent across paths.
#[derive(Debug)]
pub enum SendOutcome {
    Success,
    Failed(String),
}

impl SendOutcome {
    pub fn to_audit_metadata(&self, to: &str, from: &str) -> serde_json::Value {
        match self {
            SendOutcome::Success => serde_json::json!({
                "result": "success",
                "to": to,
                "from": from,
                "channel": "branded",
            }),
            SendOutcome::Failed(msg) => serde_json::json!({
                "result": "failed",
                "to": to,
                "from": from,
                "channel": "branded",
                "error": msg,
            }),
        }
    }
}

/// Build the From mailbox honouring `from_name` (display name) when set.
fn from_mailbox(row: &BrandingRow) -> Result<Mailbox> {
    let addr: Mailbox = row
        .from_addr
        .parse()
        .map_err(|e| anyhow!("invalid from_addr `{}`: {e}", row.from_addr))?;
    Ok(match &row.from_name {
        Some(name) if !name.trim().is_empty() => Mailbox::new(Some(name.clone()), addr.email),
        _ => addr,
    })
}

/// Send `subject` + plain-text `body` to `to_addr` using the tenant's
/// branded SMTP transport. The body is sent as text/plain — the
/// `footer_html` field on the row is documented but rendering it
/// requires building a multipart message; that's a follow-up. We do
/// honour it as a plain-text appendix for now so operators can verify
/// the column round-trips.
pub async fn send_branded(
    cache: &TransportCache,
    row: &BrandingRow,
    to_addr: &str,
    subject: &str,
    body: &str,
) -> SendOutcome {
    let from = match from_mailbox(row) {
        Ok(m) => m,
        Err(e) => return SendOutcome::Failed(e.to_string()),
    };
    let to: Mailbox = match to_addr.parse() {
        Ok(m) => m,
        Err(e) => return SendOutcome::Failed(format!("invalid to address: {e}")),
    };

    let mut builder = Message::builder()
        .from(from.clone())
        .to(to)
        .subject(subject);
    if let Some(rt) = &row.reply_to {
        if !rt.trim().is_empty() {
            match rt.parse::<Mailbox>() {
                Ok(m) => builder = builder.reply_to(m),
                Err(e) => {
                    return SendOutcome::Failed(format!("invalid reply_to `{rt}`: {e}"));
                }
            }
        }
    }

    let mut full_body = body.to_string();
    if let Some(footer) = &row.footer_html {
        if !footer.trim().is_empty() {
            full_body.push_str("\n\n---\n");
            full_body.push_str(footer);
        }
    }

    let msg = match builder.body(full_body) {
        Ok(m) => m,
        Err(e) => return SendOutcome::Failed(format!("build message: {e}")),
    };

    let transport = match cache.get_or_build(row).await {
        Ok(t) => t,
        Err(e) => return SendOutcome::Failed(e.to_string()),
    };

    match transport.send(msg).await {
        Ok(_) => SendOutcome::Success,
        Err(e) => SendOutcome::Failed(e.to_string()),
    }
}

/// Basic email syntactic check. We deliberately do not pull in a heavy
/// email-parser crate; this matches `<local>@<host>` with at least one
/// dot in the host. Display names like `"Acme" <a@b.com>` are validated
/// by `lettre`'s `Mailbox` parser at send time — this is only the
/// admin-side guardrail to prevent obvious typos.
pub fn looks_like_email(s: &str) -> bool {
    // Strip an optional display name (`Foo <a@b>`) and check the inner.
    let core = if let (Some(a), Some(b)) = (s.find('<'), s.rfind('>')) {
        if a < b {
            &s[a + 1..b]
        } else {
            s
        }
    } else {
        s
    };
    let parts: Vec<&str> = core.splitn(2, '@').collect();
    if parts.len() != 2 {
        return false;
    }
    let (local, host) = (parts[0], parts[1]);
    !local.is_empty()
        && !host.is_empty()
        && host.contains('.')
        && !host.starts_with('.')
        && !host.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serialise tests that mutate the global env. Without this the
    /// cargo test runner's threadpool races on `SKILL_POOL_EMAIL_SECRET_KEY`
    /// and one test's setvar clobbers another's expectations.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_key<F: FnOnce()>(key_hex: &str, f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(ENCRYPTION_KEY_ENV).ok();
        std::env::set_var(ENCRYPTION_KEY_ENV, key_hex);
        f();
        match prev {
            Some(v) => std::env::set_var(ENCRYPTION_KEY_ENV, v),
            None => std::env::remove_var(ENCRYPTION_KEY_ENV),
        }
    }

    fn without_key<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var(ENCRYPTION_KEY_ENV).ok();
        std::env::remove_var(ENCRYPTION_KEY_ENV);
        f();
        if let Some(v) = prev {
            std::env::set_var(ENCRYPTION_KEY_ENV, v);
        }
    }

    #[test]
    fn encrypt_decrypt_roundtrip_with_key() {
        let key_hex = "0".repeat(64);
        with_key(&key_hex, || {
            let blob = encrypt_password("hunter2");
            // Must NOT contain the plaintext anywhere.
            assert!(!blob.windows(7).any(|w| w == b"hunter2"));
            assert_eq!(blob[0], FMT_AES256_GCM_V1);
            let pt = decrypt_password(&blob).expect("decrypt");
            assert_eq!(pt, "hunter2");
        });
    }

    #[test]
    fn fallback_when_key_unset_marked_v0() {
        without_key(|| {
            let blob = encrypt_password("plain");
            assert_eq!(blob[0], FMT_PLAINTEXT_B64_V0);
            let pt = decrypt_password(&blob).expect("decrypt fallback");
            assert_eq!(pt, "plain");
        });
    }

    #[test]
    fn nonces_differ_so_ciphertexts_differ() {
        let key_hex = "1".repeat(64);
        with_key(&key_hex, || {
            let a = encrypt_password("same");
            let b = encrypt_password("same");
            assert_ne!(a, b, "GCM nonces must randomise output");
        });
    }

    #[test]
    fn inject_password_rewrites_userinfo() {
        let out = inject_password("smtps://user@smtp.example.com:465", "pw!").unwrap();
        // `url` percent-encodes special chars.
        assert!(out.starts_with("smtps://user:"));
        assert!(out.ends_with("@smtp.example.com:465"));
        assert!(out.contains("pw"));
    }

    #[test]
    fn looks_like_email_accepts_basic_addresses() {
        assert!(looks_like_email("a@b.com"));
        assert!(looks_like_email("first.last@sub.example.org"));
        assert!(looks_like_email("Acme <noreply@acme.example.com>"));
    }

    #[test]
    fn looks_like_email_rejects_obvious_junk() {
        assert!(!looks_like_email(""));
        assert!(!looks_like_email("nope"));
        assert!(!looks_like_email("@b.com"));
        assert!(!looks_like_email("a@"));
        assert!(!looks_like_email("a@b"));
        assert!(!looks_like_email("a@.com"));
    }
}
