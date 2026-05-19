//! Redis client wrapper.
//!
//! **NOTE (worktree B / rate-limiter):** This module is the contract the
//! rate-limiter writes against — `Redis` is an alias for the async
//! `ConnectionManager` type, and `connect` opens a managed connection
//! from a URL. Sister-agent A owns the full implementation (richer
//! `cached_json`, eviction hooks, metrics). At merge time A's version of
//! this file replaces the stub below. The public surface this file
//! exposes (`Redis`, `connect`) is the bare minimum the rate-limiter
//! needs and is identical to what A ships, so the merge is mechanical.

use anyhow::{Context, Result};

/// Process-wide Redis handle. `ConnectionManager` is multiplex-safe —
/// callers clone it cheaply and share across tasks.
pub type Redis = redis::aio::ConnectionManager;

/// Open a managed connection to `url` (e.g. `redis://127.0.0.1:6379`).
///
/// Failure here is non-fatal in `AppState::new` — the caller logs and
/// continues with `None`. Every consumer must handle the "no Redis"
/// path (rate-limiter fails open, etc.).
pub async fn connect(url: &str) -> Result<Redis> {
    let client = redis::Client::open(url).with_context(|| format!("parse redis url {url}"))?;
    let manager = redis::aio::ConnectionManager::new(client)
        .await
        .with_context(|| format!("connect to redis at {url}"))?;
    Ok(manager)
}
