//! Integration tests for the Redis-backed job queue (#10 §D).
//!
//! Real Redis via `testcontainers`. Six tests:
//!
//!   1. Enqueue → dequeue → ack — round-trips the happy path.
//!   2. Idempotency — same key within 24h returns `Deduped`.
//!   3. Retry with exponential back-off — score is `now+1000ms`, then
//!      `now+2000ms`.
//!   4. DLQ after `max_attempts` — six dequeues+nacks lands the job in
//!      `q:<n>:dlq` and the seventh `try_dequeue` is `None`.
//!   5. Lease expiry — a dequeue without ack re-appears via a manual
//!      score-rewind (we don't want to sleep 60s in CI).
//!   6. Graceful fallback — `notify::draft_created` still works when
//!      Redis is `None` (regression test on the inline path).
//!
//! The graceful-fallback test exercises the existing inline send path
//! through the public API and is the test that guarantees the legacy
//! behaviour was preserved by the queue refactor.

use std::net::SocketAddr;
use std::time::Duration;

use anyhow::Result;
use bytes::Bytes;
use flate2::write::GzEncoder;
use flate2::Compression;
use reqwest::multipart::{Form, Part};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::postgres::PgPoolOptions;
use std::io::Write;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::redis::Redis as RedisContainer;

use skill_pool_server::cache;
use skill_pool_server::queue::{EnqueueOutcome, Job, NackOutcome, Queue};

// ---------------------------------------------------------------------------
// Test fixture: a tiny Job that records its idempotency key explicitly so
// each test can pick a fresh one and not collide with another test's
// 24h dedup marker.
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
struct DemoJob {
    id: String,
    note: String,
    #[serde(default)]
    max_attempts_override: Option<u32>,
}

impl Job for DemoJob {
    const KIND: &'static str = "demo";
    fn idempotency_key(&self) -> String {
        self.id.clone()
    }
    fn max_attempts(&self) -> u32 {
        self.max_attempts_override.unwrap_or(5)
    }
}

async fn boot_redis() -> Result<(
    cache::Redis,
    testcontainers::ContainerAsync<RedisContainer>,
)> {
    let r = RedisContainer::default().start().await?;
    let port = r.get_host_port_ipv4(6379).await?;
    let url = format!("redis://127.0.0.1:{port}");
    let redis = cache::connect(&url).await?;
    Ok((redis, r))
}

/// Pick a unique queue name per test so a parallel test run doesn't see
/// each other's ZSET / DLQ contents. (testcontainers gives each test
/// its own Redis container anyway, but this is belt-and-braces and
/// keeps the namespace human-readable.)
fn unique_queue_name(prefix: &str) -> &'static str {
    // Leak a `&'static str` — fine for tests, each Queue takes one.
    let s = format!("test-{prefix}-{}", uuid_str_short());
    Box::leak(s.into_boxed_str())
}

fn uuid_str_short() -> String {
    // We don't need cryptographic strength here, just uniqueness
    // within a test run. The first 8 hex digits of a v4 are plenty.
    uuid::Uuid::new_v4().simple().to_string()[..8].to_string()
}

// ---------------------------------------------------------------------------
// 1. Enqueue → dequeue → ack
// ---------------------------------------------------------------------------

#[tokio::test]
async fn enqueue_dequeue_ack_round_trip() -> Result<()> {
    let (redis, _r) = boot_redis().await?;
    let name = unique_queue_name("happy");
    let q = Queue::new(redis, name);

    let j = DemoJob {
        id: "j1".into(),
        note: "hello".into(),
        max_attempts_override: None,
    };

    assert_eq!(q.enqueue(&j).await?, EnqueueOutcome::Enqueued);

    let due = q.try_dequeue().await?.expect("should have a due job");
    assert_eq!(due.kind, "demo");
    assert_eq!(due.attempts, 0, "attempts must be 0 on first dequeue");
    assert_eq!(due.max_attempts, 5);
    let payload: DemoJob = serde_json::from_value(due.payload.clone())?;
    assert_eq!(payload.note, "hello");

    // Ack: queue depth drops to zero.
    q.ack(&due.id).await?;
    assert_eq!(q.depth().await?, 0, "ack should clear the ZSET entry");
    assert!(q.try_dequeue().await?.is_none());

    Ok(())
}

// ---------------------------------------------------------------------------
// 2. Idempotency: same key twice within 24h returns Deduped on the second.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn idempotent_enqueue_dedupes() -> Result<()> {
    let (redis, _r) = boot_redis().await?;
    let name = unique_queue_name("idem");
    let q = Queue::new(redis, name);

    let j = DemoJob {
        id: "shared-key".into(),
        note: "first".into(),
        max_attempts_override: None,
    };
    let j2 = DemoJob {
        id: "shared-key".into(),
        note: "second".into(),
        max_attempts_override: None,
    };

    assert_eq!(q.enqueue(&j).await?, EnqueueOutcome::Enqueued);
    assert_eq!(q.enqueue(&j2).await?, EnqueueOutcome::Deduped);

    // Only one job is sitting in the queue.
    assert_eq!(q.depth().await?, 1);

    let due = q.try_dequeue().await?.expect("first job is due");
    let payload: DemoJob = serde_json::from_value(due.payload)?;
    assert_eq!(payload.note, "first", "the deduped second job must not overwrite the first");

    Ok(())
}

// ---------------------------------------------------------------------------
// 3. Retry with exponential back-off: first nack → next score is now+1s,
//    second nack → now+2s.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn retry_uses_exponential_backoff() -> Result<()> {
    let (redis, _r) = boot_redis().await?;
    let name = unique_queue_name("backoff");
    let q = Queue::new(redis.clone(), name);

    let j = DemoJob {
        id: "to-fail".into(),
        note: "n".into(),
        max_attempts_override: Some(10), // plenty of headroom so we don't DLQ
    };
    q.enqueue(&j).await?;

    let due = q.try_dequeue().await?.expect("ready");
    let now_before_nack = unix_ms();
    let outcome = q.nack(&due.id, "boom").await?;
    let (next_run_at_1, attempts_1) = match outcome {
        NackOutcome::Retrying { next_run_at, attempts } => (next_run_at, attempts),
        _ => panic!("expected Retrying"),
    };
    assert_eq!(attempts_1, 1, "attempts increments to 1 on first nack");

    // ZSET score should be roughly now + 1_000 ms.
    let score_1 = zset_score(&redis, name, &due.id).await?;
    let next_run_at_1_ms = system_time_ms(next_run_at_1);
    assert_approx(
        next_run_at_1_ms,
        now_before_nack + 1_000,
        300,
        "first retry should be ~1s out",
    );
    assert_approx(
        score_1 as u64,
        next_run_at_1_ms,
        50,
        "ZSET score must equal nack's reported next_run_at",
    );

    // Rewind the score so the next dequeue sees the job immediately.
    // (We do not want to sleep 1s in a unit test.)
    rewind_score(&redis, name, &due.id).await?;

    let due2 = q.try_dequeue().await?.expect("retry is ready after rewind");
    assert_eq!(due2.attempts, 1, "second dequeue sees attempts=1");
    let now_before_nack2 = unix_ms();
    let outcome2 = q.nack(&due2.id, "boom2").await?;
    let attempts_2 = match outcome2 {
        NackOutcome::Retrying { attempts, .. } => attempts,
        _ => panic!("expected Retrying"),
    };
    assert_eq!(attempts_2, 2);
    let score_2 = zset_score(&redis, name, &due2.id).await?;
    assert_approx(
        score_2 as u64,
        now_before_nack2 + 2_000,
        300,
        "second retry should be ~2s out",
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 4. DLQ after max_attempts. With max=5, the FIFTH nack should land the
//    job in the DLQ. The next try_dequeue must return None.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dlq_after_max_attempts() -> Result<()> {
    let (redis, _r) = boot_redis().await?;
    let name = unique_queue_name("dlq");
    let q = Queue::new(redis.clone(), name);

    let j = DemoJob {
        id: "doomed".into(),
        note: "x".into(),
        max_attempts_override: Some(5),
    };
    q.enqueue(&j).await?;

    let mut last_outcome = None;
    let mut last_id: Option<String> = None;
    for i in 1..=5 {
        // After the first nack the job's score has been pushed into
        // the future by the back-off; rewind it so we can keep
        // dequeueing without waiting in real time.
        if let Some(id) = &last_id {
            rewind_score(&redis, name, id).await?;
        }
        let due = q
            .try_dequeue()
            .await?
            .unwrap_or_else(|| panic!("expected a due job on attempt {i}"));
        last_id = Some(due.id.clone());
        let outcome = q.nack(&due.id, &format!("fail {i}")).await?;
        last_outcome = Some(outcome);
    }

    assert!(
        matches!(last_outcome, Some(NackOutcome::MovedToDlq)),
        "5th nack with max=5 should land in DLQ; got {:?}",
        last_outcome
    );

    // Main queue is empty.
    assert!(q.try_dequeue().await?.is_none());
    assert_eq!(q.depth().await?, 0);
    // DLQ has exactly one entry.
    assert_eq!(q.dlq_depth().await?, 1);

    // The envelope is still inspectable (TTL hasn't fired).
    let mut conn = (*redis).clone();
    use redis::AsyncCommands;
    let id: Vec<String> = conn.lrange(format!("q:{name}:dlq"), 0, -1).await?;
    assert_eq!(id.len(), 1);
    let json: Option<String> = conn.get(format!("q:{name}:job:{}", id[0])).await?;
    assert!(json.is_some(), "DLQ job envelope should remain for inspection");

    Ok(())
}

// ---------------------------------------------------------------------------
// 5. Lease expiry: dequeue without ack, then re-dequeue once the lease
//    has expired. attempts is unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn lease_expiry_redelivers_with_same_attempts() -> Result<()> {
    let (redis, _r) = boot_redis().await?;
    let name = unique_queue_name("lease");
    let q = Queue::new(redis.clone(), name);

    let j = DemoJob {
        id: "lease-test".into(),
        note: "y".into(),
        max_attempts_override: Some(10),
    };
    q.enqueue(&j).await?;

    let due1 = q.try_dequeue().await?.expect("ready");
    assert_eq!(due1.attempts, 0);

    // Worker crashed: don't ack/nack. The lease scored the member at
    // now + 60_000ms. To avoid sleeping 60s in CI, push the score
    // back to "now" — the same effect the wall clock would have.
    rewind_score(&redis, name, &due1.id).await?;

    let due2 = q.try_dequeue().await?.expect("redeliver after lease");
    assert_eq!(due2.id, due1.id, "same job_id (same logical job)");
    assert_eq!(
        due2.attempts, 0,
        "attempts must not increment on lease-expiry redelivery"
    );

    Ok(())
}

// ---------------------------------------------------------------------------
// 6. Graceful fallback: with `redis_url = None`, the email-notification
//    inline path still runs and writes the audit row. This is a
//    regression test for the contract that the no-Redis deploy isn't
//    affected by the queue refactor.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn graceful_fallback_inline_email_send_when_no_redis() -> Result<()> {
    use skill_pool_server::{admin, config, routes, state};

    let pg = Postgres::default()
        .with_name("pgvector/pgvector")
        .with_tag("pg16")
        .start()
        .await?;
    let port = pg.get_host_port_ipv4(5432).await?;
    let db_url = format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres");
    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect(&db_url)
        .await?;
    sqlx::migrate!("./migrations").run(&pool).await?;

    let storage_dir = tempfile::tempdir()?;
    let storage_uri = format!("fs://{}", storage_dir.path().display());

    admin::create_tenant(&pool, "acme", "Acme", "team").await?;
    let token = admin::create_token(
        &pool,
        "acme",
        "admin",
        "tenant:admin skills:read skills:publish",
    )
    .await?
    .raw_token;

    // Configure SMTP so an email job *would* be produced. The send
    // will fail (no relay on 127.0.0.1:2525) — that's fine: we're
    // checking the inline path writes the failed-delivery audit row.
    sqlx::query(
        "UPDATE tenants SET notification_smtp_url = $1, \
         notification_smtp_from = $2, notification_smtp_to = $3 \
         WHERE slug = 'acme'",
    )
    .bind("smtp://nope:25")
    .bind("noreply@example.com")
    .bind("curators@example.com")
    .execute(&pool)
    .await?;

    let cfg = config::Config {
        bind: "127.0.0.1:0".into(),
        tenancy_mode: config::TenancyModeRaw::default(),
        database_url: db_url,
        database_read_url: None,
        redis_url: None, // <-- the point of this test
        db_pool_size: 20,
        storage_uri,
        origin_pattern: "http://{tenant}.localhost".into(),
        embedding: config::EmbeddingConfig::default(),
        queue_enabled: None,
        decay_check_interval_secs: 0,
        git_repo_path: None,
    };
    let state = state::AppState::new(&cfg).await?;
    // Sanity check: no queue, no Redis.
    assert!(state.redis().is_none(), "Redis must be None for this test");
    assert!(state.queue().is_none(), "Queue must be None when Redis is None");

    let app = routes::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr: SocketAddr = listener.local_addr()?;
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    let base = format!("http://{addr}");

    let c = reqwest::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;

    let bundle = build_bundle("---\nname: x\ndescription: x\n---\n\nbody\n");
    let meta = json!({ "slug": "x", "origin": "cli" });
    let form = Form::new().text("metadata", meta.to_string()).part(
        "bundle",
        Part::bytes(bundle.to_vec()).file_name("x.tar.gz").mime_str("application/gzip")?,
    );
    let r = c
        .post(format!("{base}/v1/drafts"))
        .header("x-skill-pool-tenant", "acme")
        .bearer_auth(&token)
        .multipart(form)
        .send()
        .await?;
    assert_eq!(r.status().as_u16(), 201, "draft must be created");

    // Poll the audit log for the inline-send row.
    let mut attempts = 0;
    let audit: Option<(String, Value)> = loop {
        let row: Option<(String, Value)> = sqlx::query_as(
            "SELECT action, metadata FROM audit_events \
             WHERE action = 'notification.deliver' AND target_kind = 'email' \
             ORDER BY ts DESC LIMIT 1",
        )
        .fetch_optional(&pool)
        .await?;
        if row.is_some() || attempts > 50 {
            break row;
        }
        attempts += 1;
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let (action, metadata) = audit.expect("inline send should still write an audit row");
    assert_eq!(action, "notification.deliver");
    assert_eq!(metadata["result"], "failed");
    assert_eq!(metadata["to"], "curators@example.com");

    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn unix_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

fn system_time_ms(t: std::time::SystemTime) -> u64 {
    use std::time::UNIX_EPOCH;
    t.duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

fn assert_approx(actual: u64, expected: u64, tolerance_ms: u64, msg: &str) {
    let diff = actual.abs_diff(expected);
    assert!(
        diff <= tolerance_ms,
        "{msg}: expected ≈{expected}, got {actual} (diff {diff}ms > tol {tolerance_ms}ms)"
    );
}

async fn zset_score(redis: &cache::Redis, name: &str, id: &str) -> Result<f64> {
    use redis::AsyncCommands;
    let mut conn = (**redis).clone();
    let s: Option<f64> = conn.zscore(format!("q:{name}"), id).await?;
    s.ok_or_else(|| anyhow::anyhow!("no zscore for {id}"))
}

/// Force the only job in the queue to be due immediately. Used to
/// skip past back-off timers without sleeping.
async fn rewind_score(redis: &cache::Redis, name: &str, id: &str) -> Result<()> {
    use redis::AsyncCommands;
    let mut conn = (**redis).clone();
    let _: i64 = conn.zadd(format!("q:{name}"), id, unix_ms() as i64).await?;
    Ok(())
}

fn build_bundle(skill_md: &str) -> Bytes {
    let mut tar = tar::Builder::new(Vec::new());
    let body = skill_md.as_bytes();
    let mut header = tar::Header::new_gnu();
    header.set_path("SKILL.md").unwrap();
    header.set_size(body.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, body).unwrap();
    let tar_bytes = tar.into_inner().unwrap();
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&tar_bytes).unwrap();
    Bytes::from(gz.finish().unwrap())
}

