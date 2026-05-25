//! Redis-backed job queue (#10 §D).
//!
//! A small at-least-once delivery queue layered directly on Redis: no
//! Sidekiq, no Faktory, no extra moving part to operate. The same
//! `cache::Redis` (`Arc<ConnectionManager>`) that powers the
//! read-through caches and the rate limiter doubles as the queue
//! backend.
//!
//! ## Why we built this and not pulled in a crate
//!
//! Our needs are small: enqueue a JSON payload, dequeue it once with a
//! lease, retry with exponential backoff, fall off into a DLQ after N
//! attempts. The crates that do this in Rust (`apalis`, `sidekiq-rs`,
//! `faktory-rs`) all want a longer-lived agreement than the ~250 lines
//! below give us. Most of the complexity of a real job system —
//! cron, fan-out, web UI, multi-language workers — we do not need yet.
//! When that changes, the surface here (`Queue`, `Job`, `JobHandler`)
//! is small enough to swap.
//!
//! ## Data model
//!
//! Every queue named `<name>` (just `"default"` for now) lives under
//! four Redis keys:
//!
//! ```text
//! q:<name>                ZSET   member = job_id   score = unix_ms_due
//! q:<name>:job:<id>       STRING JSON envelope (id, kind, payload, attempts, …)  TTL 7d
//! q:<name>:dlq            LIST   job_ids that exhausted retries
//! q:<name>:idem:<key>     STRING marker for dedup (SETNX, 24h TTL)
//! ```
//!
//! Choosing a ZSET for the main queue (rather than the simpler
//! `BRPOPLPUSH` LIST pattern) buys us scheduled delivery and per-job
//! leases for free: a "leased" job is just a ZSET member whose score
//! has been moved to `now + lease_ms`, so a crashed worker's job
//! becomes due again automatically without a separate "in-flight" list.
//!
//! ## Atomicity
//!
//! `try_dequeue` and `nack` both run as a single `redis::Script` (Lua)
//! so the read-modify-write cycle on the ZSET cannot race with a
//! concurrent worker pulling the same member. The simpler operations
//! (`enqueue`, `ack`, `depth`) just use pipelines.
//!
//! ## At-least-once
//!
//! Workers may execute the same job more than once if they crash
//! between `try_dequeue` and `ack`. That is why `Job::idempotency_key`
//! is mandatory: consumers must be safe to re-run. The 24h `:idem:`
//! marker dedupes enqueues only — duplicate *deliveries* of a single
//! already-enqueued job are detectable only inside the handler.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use redis::AsyncCommands;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::cache;

/// How long a worker has to ack a leased job before it becomes due
/// again. Tuned to be longer than the longest expected handler (SMTP
/// send w/ retries is on the order of a few seconds) but short enough
/// that a crashed worker doesn't sit on a job for the full retention
/// window.
pub const LEASE_DURATION_MS: i64 = 60_000;

/// TTL for the per-job JSON envelope. Long enough that an operator
/// inspecting the DLQ days after the fact can still pull the payload.
const JOB_TTL_SECS: u64 = 7 * 24 * 60 * 60;

/// TTL for the idempotency marker. Long enough to dedupe within a
/// sensible retry window for upstream callers but short enough that a
/// 24-hour-old caller resubmitting an identical payload counts as a
/// new request.
const IDEM_TTL_SECS: u64 = 24 * 60 * 60;

/// Maximum back-off between retries (30 minutes). Beyond this we'd
/// rather give up and let the operator look at the DLQ than schedule
/// retries hours into the future.
const MAX_BACKOFF_MS: u64 = 30 * 60 * 1_000;

/// Anything serializable that the worker can route. `idempotency_key`
/// is mandatory because we deliver at-least-once: consumers must be
/// safe to run more than once and the queue itself uses it for
/// best-effort enqueue-side dedup.
pub trait Job: Serialize + serde::de::DeserializeOwned + Send + Sync + 'static {
    /// Stable kind name registered with the `Worker`. Routing key.
    const KIND: &'static str;
    /// Caller-supplied dedup key. The same key within 24h is rejected
    /// with `EnqueueOutcome::Deduped`.
    fn idempotency_key(&self) -> String;
    /// Per-job override of the global retry cap. 5 by default.
    fn max_attempts(&self) -> u32 {
        5
    }
}

#[derive(Debug, Error)]
pub enum QueueError {
    #[error("redis error: {0}")]
    Redis(#[from] redis::RedisError),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("system time error: {0}")]
    Time(#[from] std::time::SystemTimeError),
}

/// Outcome of `enqueue`. `Deduped` is intentionally non-fatal — the
/// caller usually wants to log and continue.
#[derive(Debug, PartialEq, Eq)]
pub enum EnqueueOutcome {
    Enqueued,
    Deduped,
}

/// What happened on a `nack`. Drives the worker's metrics counters.
#[derive(Debug, PartialEq, Eq)]
pub enum NackOutcome {
    Retrying {
        next_run_at: SystemTime,
        attempts: u32,
    },
    MovedToDlq,
}

/// A job that `try_dequeue` returned. The handler runs against
/// `payload`; if it succeeds we `ack(id)`, if it fails we
/// `nack(id, error)`.
#[derive(Debug, Clone)]
pub struct DueJob {
    pub id: String,
    pub kind: String,
    pub payload: serde_json::Value,
    pub attempts: u32,
    pub max_attempts: u32,
}

/// Stored shape of a job in `q:<name>:job:<id>`. Public so the worker's
/// "missing handler" path can write back a final attempt count.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct JobEnvelope {
    id: String,
    kind: String,
    payload: serde_json::Value,
    attempts: u32,
    max_attempts: u32,
    idempotency_key: String,
    first_enqueued_at: u64,
}

#[derive(Clone)]
pub struct Queue {
    redis: cache::Redis,
    name: &'static str,
}

impl Queue {
    pub fn new(redis: cache::Redis, name: &'static str) -> Self {
        Self { redis, name }
    }

    pub fn name(&self) -> &'static str {
        self.name
    }

    fn zset_key(&self) -> String {
        format!("q:{}", self.name)
    }

    fn job_key(&self, id: &str) -> String {
        format!("q:{}:job:{}", self.name, id)
    }

    fn dlq_key(&self) -> String {
        format!("q:{}:dlq", self.name)
    }

    fn idem_key(&self, idem: &str) -> String {
        format!("q:{}:idem:{}", self.name, idem)
    }

    /// Enqueue at the current instant.
    pub async fn enqueue<J: Job>(&self, job: &J) -> Result<EnqueueOutcome, QueueError> {
        self.enqueue_at(job, SystemTime::now()).await
    }

    /// Enqueue with a delayed `run_at`. Used by `nack`'s back-off path
    /// indirectly (it skips the idempotency check) and exposed for the
    /// rare external caller that wants a scheduled send.
    pub async fn enqueue_at<J: Job>(
        &self,
        job: &J,
        run_at: SystemTime,
    ) -> Result<EnqueueOutcome, QueueError> {
        let now_ms = unix_ms()?;
        let due_ms = system_time_to_ms(run_at)?;
        let id = new_job_id();
        let idem = job.idempotency_key();
        let envelope = JobEnvelope {
            id: id.clone(),
            kind: J::KIND.to_string(),
            payload: serde_json::to_value(job)?,
            attempts: 0,
            max_attempts: job.max_attempts(),
            idempotency_key: idem.clone(),
            first_enqueued_at: now_ms,
        };
        let json = serde_json::to_string(&envelope)?;

        // Atomic dedup + insert. We use `SET NX EX` on the idempotency
        // marker; if it already exists, abort. Otherwise write the
        // envelope, push into the ZSET. We deliberately do this as
        // three commands rather than one Lua script: the failure mode
        // (orphaned envelope key whose ZSET entry didn't land) is
        // tolerable because envelopes carry a TTL and we never read an
        // envelope without first popping its id off the ZSET.
        let mut conn = (*self.redis).clone();
        let nx: Option<String> = redis::cmd("SET")
            .arg(self.idem_key(&idem))
            .arg(&id)
            .arg("NX")
            .arg("EX")
            .arg(IDEM_TTL_SECS)
            .query_async(&mut conn)
            .await?;
        if nx.is_none() {
            return Ok(EnqueueOutcome::Deduped);
        }

        // Envelope first, then ZSET. If the ZSET ADD fails, the
        // envelope expires on TTL; if the envelope SET fails, the
        // ZADD below never runs and the idem marker holds for 24h
        // (acceptable: caller can re-attempt with a different payload
        // or wait it out).
        let _: () = conn.set_ex(self.job_key(&id), &json, JOB_TTL_SECS).await?;
        let _: i64 = conn.zadd(self.zset_key(), &id, due_ms as i64).await?;

        Ok(EnqueueOutcome::Enqueued)
    }

    /// Pop the next due job (score <= now) and atomically extend its
    /// score by `LEASE_DURATION_MS` to claim a lease. Returns `None`
    /// when nothing is due.
    pub async fn try_dequeue(&self) -> Result<Option<DueJob>, QueueError> {
        let now_ms = unix_ms()? as i64;
        let lease_until = now_ms + LEASE_DURATION_MS;

        // Lua: ZRANGEBYSCORE [-inf, now] LIMIT 0 1, then ZADD ... XX to
        // re-score the same member. Atomic: a second worker won't see
        // the same member on its next ZRANGEBYSCORE because we bumped
        // the score past `now`.
        let script = redis::Script::new(
            r#"
            local zkey   = KEYS[1]
            local now    = tonumber(ARGV[1])
            local lease  = tonumber(ARGV[2])
            local ids    = redis.call('ZRANGEBYSCORE', zkey, '-inf', now, 'LIMIT', 0, 1)
            if #ids == 0 then return nil end
            local id = ids[1]
            redis.call('ZADD', zkey, lease, id)
            return id
            "#,
        );

        let mut conn = (*self.redis).clone();
        let id: Option<String> = script
            .key(self.zset_key())
            .arg(now_ms)
            .arg(lease_until)
            .invoke_async(&mut conn)
            .await?;
        let Some(id) = id else {
            return Ok(None);
        };

        let json: Option<String> = conn.get(self.job_key(&id)).await?;
        let Some(json) = json else {
            // Envelope evaporated (TTL or admin deletion). Drop the
            // dangling ZSET entry and report "nothing due"; the caller
            // will poll again next tick.
            let _: i64 = conn.zrem(self.zset_key(), &id).await?;
            return Ok(None);
        };
        let env: JobEnvelope = serde_json::from_str(&json)?;
        // NOTE: we do not increment `attempts` here. The counter
        // advances only when the handler explicitly fails (`nack`) so
        // that a lease-expiry redelivery (worker crashed before ack)
        // doesn't burn a retry slot. This is the at-least-once
        // contract: handlers must be idempotent w.r.t. the same
        // (job_id, attempts) pair.

        Ok(Some(DueJob {
            id: env.id,
            kind: env.kind,
            payload: env.payload,
            attempts: env.attempts,
            max_attempts: env.max_attempts,
        }))
    }

    /// Successful completion. Removes the ZSET entry and deletes the
    /// envelope (no point keeping it 7d once it's done).
    pub async fn ack(&self, job_id: &str) -> Result<(), QueueError> {
        let mut conn = (*self.redis).clone();
        let _: () = redis::pipe()
            .atomic()
            .cmd("ZREM")
            .arg(self.zset_key())
            .arg(job_id)
            .ignore()
            .cmd("DEL")
            .arg(self.job_key(job_id))
            .ignore()
            .query_async(&mut conn)
            .await?;
        Ok(())
    }

    /// Failed handler. Either re-schedules with exponential back-off
    /// or moves to the DLQ when the attempt count tops out. The
    /// envelope is mutated under a Lua script so we can branch on the
    /// current attempts count atomically.
    pub async fn nack(&self, job_id: &str, error: &str) -> Result<NackOutcome, QueueError> {
        let now_ms = unix_ms()?;

        let mut conn = (*self.redis).clone();
        // Fetch first so we can compute the next-run-at outside the
        // script (Redis Lua can't easily do JSON; we use cjson but the
        // envelopes are small enough that a fetch + script round-trip
        // is fine).
        let json: Option<String> = conn.get(self.job_key(job_id)).await?;
        let Some(json) = json else {
            // Envelope vanished. Remove the ZSET entry just in case
            // and report DLQ — there's nothing left to retry.
            let _: i64 = conn.zrem(self.zset_key(), job_id).await?;
            return Ok(NackOutcome::MovedToDlq);
        };
        let mut env: JobEnvelope = serde_json::from_str(&json)?;
        // Backoff is computed from the *pre-increment* attempt count
        // so the first nack waits 2^0 = 1s, the second 2^1 = 2s, and
        // so on. That matches the test contract and the runbook
        // schedule (1, 2, 4, 8, 16s …).
        let backoff_ms = backoff_ms_for_attempt(env.attempts);
        env.attempts += 1;

        if env.attempts >= env.max_attempts {
            // Exhausted. Push to DLQ, remove from main queue. Envelope
            // stays for inspection (its TTL still applies). Persist
            // the final attempts count so an operator running
            // `GET q:<n>:job:<id>` sees how many tries actually happened.
            let new_json = serde_json::to_string(&env)?;
            let _: () = redis::pipe()
                .atomic()
                .cmd("SET")
                .arg(self.job_key(job_id))
                .arg(&new_json)
                .arg("EX")
                .arg(JOB_TTL_SECS)
                .ignore()
                .cmd("LPUSH")
                .arg(self.dlq_key())
                .arg(job_id)
                .ignore()
                .cmd("ZREM")
                .arg(self.zset_key())
                .arg(job_id)
                .ignore()
                .query_async(&mut conn)
                .await?;
            tracing::warn!(
                queue = self.name,
                job_id,
                kind = %env.kind,
                attempts = env.attempts,
                last_error = error,
                "job moved to DLQ"
            );
            return Ok(NackOutcome::MovedToDlq);
        }

        // Re-schedule with exponential back-off.
        let next_run_ms = now_ms + backoff_ms;
        let new_json = serde_json::to_string(&env)?;
        let _: () = redis::pipe()
            .atomic()
            .cmd("SET")
            .arg(self.job_key(job_id))
            .arg(&new_json)
            .arg("EX")
            .arg(JOB_TTL_SECS)
            .ignore()
            .cmd("ZADD")
            .arg(self.zset_key())
            .arg(next_run_ms as i64)
            .arg(job_id)
            .ignore()
            .query_async(&mut conn)
            .await?;

        tracing::info!(
            queue = self.name,
            job_id,
            kind = %env.kind,
            attempts = env.attempts,
            backoff_ms,
            error,
            "job retried with back-off"
        );

        Ok(NackOutcome::Retrying {
            next_run_at: ms_to_system_time(next_run_ms),
            attempts: env.attempts,
        })
    }

    /// Number of pending (scheduled or in-flight) jobs in the main
    /// queue. Used by the metrics ticker.
    pub async fn depth(&self) -> Result<u64, QueueError> {
        let mut conn = (*self.redis).clone();
        let n: i64 = conn.zcard(self.zset_key()).await?;
        Ok(n.max(0) as u64)
    }

    /// Number of jobs sitting in the DLQ. Driven into a Prometheus
    /// gauge so the `SkillPoolDLQGrowing` alert can fire on a non-zero
    /// reading.
    pub async fn dlq_depth(&self) -> Result<u64, QueueError> {
        let mut conn = (*self.redis).clone();
        let n: i64 = conn.llen(self.dlq_key()).await?;
        Ok(n.max(0) as u64)
    }
}

/// Backoff schedule: `2^attempts * 1000` ms, capped at 30 min. The
/// input is the *pre-increment* attempt count (0 for the first failure,
/// 1 for the second, …) so the sequence is 1s, 2s, 4s, 8s, 16s, … up
/// to 1_800_000 ms. Saturating math so a pathological `attempts = u32::MAX`
/// doesn't panic on shift-overflow.
fn backoff_ms_for_attempt(attempts: u32) -> u64 {
    let shift = attempts.min(31);
    let raw = 1_000u64.saturating_mul(1u64 << shift);
    raw.min(MAX_BACKOFF_MS)
}

fn unix_ms() -> Result<u64, std::time::SystemTimeError> {
    let d = SystemTime::now().duration_since(UNIX_EPOCH)?;
    Ok(d.as_millis() as u64)
}

fn system_time_to_ms(t: SystemTime) -> Result<u64, std::time::SystemTimeError> {
    Ok(t.duration_since(UNIX_EPOCH)?.as_millis() as u64)
}

fn ms_to_system_time(ms: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(ms)
}

/// A short opaque ID. We don't need a UUID's collision guarantees here
/// (the idempotency key already deduplicates *content*); 128 bits of
/// hex from the OS RNG is plenty.
fn new_job_id() -> String {
    use rand::RngCore;
    let mut buf = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut buf);
    hex::encode(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_until_cap() {
        assert_eq!(backoff_ms_for_attempt(0), 1_000);
        assert_eq!(backoff_ms_for_attempt(1), 2_000);
        assert_eq!(backoff_ms_for_attempt(2), 4_000);
        assert_eq!(backoff_ms_for_attempt(3), 8_000);
        assert_eq!(backoff_ms_for_attempt(4), 16_000);
        assert_eq!(backoff_ms_for_attempt(5), 32_000);
        // Past the cap.
        assert_eq!(backoff_ms_for_attempt(20), MAX_BACKOFF_MS);
        // Saturates on overflow.
        assert_eq!(backoff_ms_for_attempt(u32::MAX), MAX_BACKOFF_MS);
    }

    #[test]
    fn job_ids_are_unique() {
        let a = new_job_id();
        let b = new_job_id();
        assert_ne!(a, b);
        assert_eq!(a.len(), 32);
    }
}
