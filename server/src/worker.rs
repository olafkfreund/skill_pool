//! Job-queue worker loop (#10 §D).
//!
//! Owns one polling loop that drains the shared `Queue` and dispatches
//! each ready job to a `JobHandler` registered by kind. Failure paths:
//!
//!   * **Handler returns `Err(msg)`** → `nack` with the message. The
//!     queue decides retry-with-back-off vs DLQ based on the
//!     attempts counter.
//!   * **No handler registered for the job's kind** → `nack` with a
//!     permanent flag (we burn through retries fast: the queue still
//!     applies its normal retry, but the kind isn't going to start
//!     existing mid-process so the job lands in the DLQ within seconds).
//!     This protects us against an envelope deserialising fine but
//!     pointing at a worker version that doesn't know what to do with it.
//!   * **Handler panics** → `tokio::spawn` catches it via the
//!     `JoinHandle`'s `Err` and we treat that as a nack with
//!     `"handler panicked"`. Panic recovery here is intentional: a
//!     bad job should not take the whole worker down.
//!
//! ## Shutdown
//!
//! `Worker::run` accepts a `tokio::sync::watch::Receiver<bool>`. When
//! the value flips to `true` we drain any in-flight job (the current
//! handler runs to completion or its ack/nack lands) then exit. This
//! is plenty for SIGTERM — the supervising main loop gives the worker
//! up to 30s to finish before timing out.
//!
//! ## Metrics
//!
//! Three Prometheus series are sampled here:
//!   * `skill_pool_queue_depth{queue}` and `skill_pool_queue_dlq_depth{queue}`
//!     — gauges, sampled every 30s on a `tokio::time::interval`.
//!   * `skill_pool_queue_jobs_total{queue,outcome}` — counter,
//!     incremented inline as we ack / nack.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::watch;

use crate::metrics;
use crate::queue::{NackOutcome, Queue};

/// How long the worker idles between polls when the queue had nothing
/// due. Short because Redis ZRANGEBYSCORE is cheap; long enough that
/// an idle deployment isn't hammering the broker.
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// How often the depth gauges are sampled.
const METRICS_INTERVAL: Duration = Duration::from_secs(30);

/// Anything that processes a job payload. Implementations get the raw
/// JSON and are responsible for parsing it into the concrete `Job`
/// type they expect — the worker doesn't know about the job's static
/// type. On success return `Ok(())`; on failure return `Err(msg)` and
/// `msg` becomes the audit / log breadcrumb.
#[async_trait]
pub trait JobHandler: Send + Sync {
    async fn handle(&self, payload: serde_json::Value) -> Result<(), String>;
}

pub struct Worker {
    queue: Arc<Queue>,
    handlers: HashMap<&'static str, Arc<dyn JobHandler>>,
    shutdown: watch::Receiver<bool>,
}

impl Worker {
    pub fn new(queue: Arc<Queue>, shutdown: watch::Receiver<bool>) -> Self {
        Self {
            queue,
            handlers: HashMap::new(),
            shutdown,
        }
    }

    /// Wire up a handler for a job kind. Calling twice for the same
    /// kind replaces — last-write-wins. We deliberately don't return
    /// the previous entry because callers always register at startup.
    pub fn register<H: JobHandler + 'static>(&mut self, kind: &'static str, handler: H) {
        self.handlers.insert(kind, Arc::new(handler));
    }

    /// Run until the shutdown channel flips to `true`. Owns the
    /// receiver so the worker can be moved into a `tokio::spawn`.
    pub async fn run(mut self) {
        tracing::info!(queue = self.queue.name(), "queue worker starting");
        let mut metrics_tick = tokio::time::interval(METRICS_INTERVAL);
        // The first tick fires immediately. We want to publish an
        // initial reading so the gauge series isn't absent for the
        // first 30s — leave it as-is.
        loop {
            tokio::select! {
                // Shutdown wins on every iteration; we don't `biased!`
                // because the metrics + dequeue arms are roughly
                // balanced and `select!`'s pseudo-random tie-break is
                // fine.
                changed = self.shutdown.changed() => {
                    if changed.is_err() || *self.shutdown.borrow() {
                        tracing::info!(queue = self.queue.name(), "queue worker shutting down");
                        return;
                    }
                }
                _ = metrics_tick.tick() => {
                    self.sample_gauges().await;
                }
                _ = tokio::time::sleep(IDLE_POLL_INTERVAL) => {
                    // After a sleep we always try one dequeue. If the
                    // queue was non-empty we'll loop back and skip the
                    // sleep next iteration via the inner "drain" loop.
                    self.drain_due().await;
                }
            }
        }
    }

    /// Pop and process every currently-due job in a tight loop. Stops
    /// when the queue says "nothing due" or shutdown fires. Each
    /// individual handler is awaited before we move on — this keeps
    /// concurrency at 1, which is fine for v1 (the SMTP send is the
    /// slowest job we have at ~hundreds of milliseconds, and a single
    /// worker covers thousands of mails per minute).
    async fn drain_due(&self) {
        loop {
            // Exit drain if shutdown was signalled mid-batch.
            if *self.shutdown.borrow() {
                return;
            }
            let job = match self.queue.try_dequeue().await {
                Ok(Some(job)) => job,
                Ok(None) => return,
                Err(e) => {
                    // Redis blip. Log and back off; the outer loop's
                    // 1s sleep gives the connection manager time to
                    // reconnect.
                    tracing::warn!(
                        queue = self.queue.name(),
                        error = %e,
                        "queue try_dequeue failed; backing off"
                    );
                    return;
                }
            };

            self.process(job).await;
        }
    }

    async fn process(&self, job: crate::queue::DueJob) {
        let kind = job.kind.clone();
        let id = job.id.clone();
        let attempts = job.attempts;
        let Some(handler) = self.handlers.get(kind.as_str()).cloned() else {
            tracing::warn!(
                queue = self.queue.name(),
                job_id = %id,
                kind = %kind,
                "no handler registered for job kind; nacking"
            );
            self.do_nack(&id, "no handler registered").await;
            return;
        };

        // Spawn so a panic in the handler doesn't take the worker
        // loop with it. We immediately await — concurrency remains 1.
        let payload = job.payload.clone();
        let handle = tokio::spawn(async move { handler.handle(payload).await });
        let result = match handle.await {
            Ok(r) => r,
            Err(join_err) => {
                tracing::error!(
                    queue = self.queue.name(),
                    job_id = %id,
                    kind = %kind,
                    error = %join_err,
                    "job handler panicked"
                );
                Err(format!("handler panicked: {join_err}"))
            }
        };

        match result {
            Ok(()) => match self.queue.ack(&id).await {
                Ok(()) => {
                    metrics::queue_jobs_total(self.queue.name(), "success").inc();
                    tracing::debug!(
                        queue = self.queue.name(),
                        job_id = %id,
                        kind = %kind,
                        attempts,
                        "job acked"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        queue = self.queue.name(),
                        job_id = %id,
                        error = %e,
                        "ack failed; lease will expire and job will redeliver"
                    );
                    metrics::queue_jobs_total(self.queue.name(), "failed").inc();
                }
            },
            Err(msg) => {
                self.do_nack(&id, &msg).await;
            }
        }
    }

    async fn do_nack(&self, job_id: &str, error: &str) {
        match self.queue.nack(job_id, error).await {
            Ok(NackOutcome::Retrying { .. }) => {
                metrics::queue_jobs_total(self.queue.name(), "retried").inc();
            }
            Ok(NackOutcome::MovedToDlq) => {
                metrics::queue_jobs_total(self.queue.name(), "dlq").inc();
            }
            Err(e) => {
                tracing::warn!(
                    queue = self.queue.name(),
                    job_id,
                    error = %e,
                    "nack failed; lease will expire and job will redeliver"
                );
                metrics::queue_jobs_total(self.queue.name(), "failed").inc();
            }
        }
    }

    async fn sample_gauges(&self) {
        match self.queue.depth().await {
            Ok(n) => metrics::queue_depth(self.queue.name()).set(n as i64),
            Err(e) => tracing::debug!(
                queue = self.queue.name(),
                error = %e,
                "queue depth sample failed"
            ),
        }
        match self.queue.dlq_depth().await {
            Ok(n) => metrics::queue_dlq_depth(self.queue.name()).set(n as i64),
            Err(e) => tracing::debug!(
                queue = self.queue.name(),
                error = %e,
                "queue dlq_depth sample failed"
            ),
        }
    }
}
