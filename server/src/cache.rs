//! Redis read-through cache (issue #10 §A, #9 §L36).
//!
//! Thin async wrapper on `redis::aio::ConnectionManager` that the
//! server-side hot paths (theme resolution, per-request auth lookup,
//! rate limiter) use to dodge a DB round-trip on every request.
//!
//! Design principles:
//!
//!   1. **Graceful fallback.** Redis is *optional*. `AppState::redis()`
//!      returns `Option<&Redis>`; callers branch on that and run their
//!      loader directly when it's `None`. A transient error from a
//!      live client is *logged* and the loader runs — Redis being
//!      unhealthy must never propagate to the user.
//!
//!   2. **Cheap clones.** `Redis = Arc<ConnectionManager>` — moving it
//!      across an `.await` is a refcount bump, not a connection.
//!
//!   3. **JSON encoding.** Cached values are `serde_json::to_string`'d.
//!      Compact, debuggable with `redis-cli`, and version-tolerant
//!      because we never serialize `bincode`-ish raw representations
//!      that would change on a Rust struct edit. Key prefixes carry a
//!      version (`theme:v1:…`, `auth:v1:…`) so a breaking format
//!      change is a key-prefix bump rather than a flush.
//!
//!   4. **No miss caching for auth.** See `auth.rs` for the policy:
//!      a 401 today might be a token minted ten seconds from now, so
//!      we never cache negative auth answers. Theme misses *are* cached
//!      because a tenant with no theme row is the default state for
//!      brand-new tenants and won't flip every second.

use std::sync::Arc;

use anyhow::Result;
use redis::aio::ConnectionManager;
use redis::AsyncCommands;
use serde::{de::DeserializeOwned, Serialize};

pub type Redis = Arc<ConnectionManager>;

/// Build a connection manager from a URL. Accepts both `redis://` and
/// `rediss://` (TLS — used by ElastiCache in transit-encryption mode).
///
/// The manager owns one multiplexed connection that all callers share;
/// it reconnects automatically when the link drops.
pub async fn connect(url: &str) -> Result<Redis> {
    let client = redis::Client::open(url)?;
    let mgr = ConnectionManager::new(client).await?;
    Ok(Arc::new(mgr))
}

/// Read-through cache helper.
///
/// 1. `GET key`. If hit, deserialize and return.
/// 2. On miss (or any Redis error), run `loader`.
/// 3. Best-effort `SET key value EX ttl_seconds` — ignore failure.
///
/// Errors from Redis itself never propagate; they are logged at WARN
/// and the cache becomes a no-op for this request. Errors from the
/// loader propagate as-is.
pub async fn cached_json<T, F, Fut>(
    redis: &Redis,
    key: &str,
    ttl_seconds: usize,
    loader: F,
) -> Result<T>
where
    T: Serialize + DeserializeOwned,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    // Cloning the Arc<ConnectionManager> is cheap; we need a `mut` clone
    // because the redis driver's high-level commands take `&mut self`.
    let mut conn = (**redis).clone();

    // GET
    let cached: Option<String> = match conn.get(key).await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, key, "redis GET failed; bypassing cache");
            None
        }
    };

    if let Some(s) = cached {
        match serde_json::from_str::<T>(&s) {
            Ok(v) => return Ok(v),
            Err(e) => {
                // Stale schema. Log + treat as miss; the SETEX below
                // will overwrite with the new shape.
                tracing::warn!(error = %e, key, "redis cached value failed to deserialize; refreshing");
            }
        }
    }

    // Miss → loader → write-back.
    let value = loader().await?;

    match serde_json::to_string(&value) {
        Ok(s) => {
            // `set_ex` is the SETEX wrapper; ttl is in seconds.
            let r: redis::RedisResult<()> = conn.set_ex(key, s, ttl_seconds as u64).await;
            if let Err(e) = r {
                tracing::warn!(error = %e, key, "redis SETEX failed; cache write skipped");
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, key, "value did not serialize for cache; skipping write");
        }
    }

    Ok(value)
}

/// Delete a single key. Best-effort: errors are logged, not propagated,
/// because invalidation is paired with a successful DB write that's
/// already committed — refusing to return the user a 200 because the
/// cache flush hiccuped would be the wrong move.
pub async fn invalidate(redis: &Redis, key: &str) -> Result<()> {
    let mut conn = (**redis).clone();
    if let Err(e) = conn.del::<_, i64>(key).await {
        tracing::warn!(error = %e, key, "redis DEL failed; entry will expire on TTL");
    }
    Ok(())
}

/// Delete every key matching `prefix*`. Uses `SCAN` with a `MATCH`
/// pattern (not `KEYS` — `KEYS` blocks the server, fatal for an
/// embedded cache shared with the rate limiter).
///
/// Best-effort like `invalidate`. The blast radius is bounded because
/// callers always use a tenant-scoped prefix (e.g. `theme:v1:<uuid>`).
pub async fn invalidate_prefix(redis: &Redis, prefix: &str) -> Result<()> {
    let mut conn = (**redis).clone();
    let pattern = format!("{prefix}*");
    let mut cursor: u64 = 0;
    loop {
        let res: redis::RedisResult<(u64, Vec<String>)> = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg(&pattern)
            .arg("COUNT")
            .arg(256)
            .query_async(&mut conn)
            .await;
        let (next, keys) = match res {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, prefix, "redis SCAN failed; aborting prefix invalidation");
                return Ok(());
            }
        };
        if !keys.is_empty() {
            if let Err(e) = conn.del::<_, i64>(&keys).await {
                tracing::warn!(error = %e, prefix, "redis DEL (prefix) failed");
            }
        }
        if next == 0 {
            break;
        }
        cursor = next;
    }
    Ok(())
}
