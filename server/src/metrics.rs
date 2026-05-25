//! Prometheus metrics for the skill-pool server.
//!
//! Exposes four instrument families:
//!
//! * `http_requests_total`          — counter, labelled by method + path + status
//! * `http_request_duration_seconds` — histogram, same labels
//! * `db_pool_size`                 — gauge, current sqlx pool size
//! * `http_requests_in_flight`      — gauge, concurrent in-progress requests
//!
//! The `/metrics` handler renders the default registry in Prometheus text
//! exposition format 0.0.4.  No authentication is required (same posture
//! as `/v1/healthz`).

use std::time::Instant;

use axum::body::Body;
use axum::extract::{MatchedPath, Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use prometheus::{
    register_histogram_vec, register_int_counter_vec, register_int_gauge, register_int_gauge_vec,
    Encoder, HistogramVec, IntCounter, IntCounterVec, IntGauge, IntGaugeVec, TextEncoder,
};

use crate::state::AppState;

// ---------------------------------------------------------------------------
// Lazily-initialised global instruments
// ---------------------------------------------------------------------------

fn http_requests_total() -> &'static IntCounterVec {
    static ONCE: std::sync::OnceLock<IntCounterVec> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_int_counter_vec!(
            "http_requests_total",
            "Total number of HTTP requests handled",
            &["method", "path", "status"]
        )
        .expect("register http_requests_total")
    })
}

fn http_request_duration_seconds() -> &'static HistogramVec {
    static ONCE: std::sync::OnceLock<HistogramVec> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_histogram_vec!(
            "http_request_duration_seconds",
            "HTTP request latency in seconds",
            &["method", "path", "status"],
            vec![0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
        )
        .expect("register http_request_duration_seconds")
    })
}

fn http_requests_in_flight() -> &'static IntGauge {
    static ONCE: std::sync::OnceLock<IntGauge> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_int_gauge!(
            "http_requests_in_flight",
            "Number of HTTP requests currently being processed"
        )
        .expect("register http_requests_in_flight")
    })
}

fn db_pool_size() -> &'static IntGauge {
    static ONCE: std::sync::OnceLock<IntGauge> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_int_gauge!(
            "db_pool_size",
            "Current number of connections in the sqlx pool"
        )
        .expect("register db_pool_size")
    })
}

// ---------------------------------------------------------------------------
// Job-queue instruments (#10 §D)
// ---------------------------------------------------------------------------
//
// Three series, all labelled by the queue name (we only run `default`
// today, but worker shipped with a label keeps the API stable when a
// second queue arrives):
//
//   * `skill_pool_queue_depth{queue}`     — current ZCARD of q:<name>
//   * `skill_pool_queue_dlq_depth{queue}` — current LLEN of q:<name>:dlq
//   * `skill_pool_queue_jobs_total{queue,outcome}` — counter, outcome ∈
//     {success, retried, dlq, failed}

fn queue_depth_vec() -> &'static IntGaugeVec {
    static ONCE: std::sync::OnceLock<IntGaugeVec> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_int_gauge_vec!(
            "skill_pool_queue_depth",
            "Current number of pending or in-flight jobs in the queue (ZCARD)",
            &["queue"]
        )
        .expect("register skill_pool_queue_depth")
    })
}

fn queue_dlq_depth_vec() -> &'static IntGaugeVec {
    static ONCE: std::sync::OnceLock<IntGaugeVec> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_int_gauge_vec!(
            "skill_pool_queue_dlq_depth",
            "Current number of jobs in the dead-letter list (LLEN)",
            &["queue"]
        )
        .expect("register skill_pool_queue_dlq_depth")
    })
}

fn queue_jobs_total_vec() -> &'static IntCounterVec {
    static ONCE: std::sync::OnceLock<IntCounterVec> = std::sync::OnceLock::new();
    ONCE.get_or_init(|| {
        register_int_counter_vec!(
            "skill_pool_queue_jobs_total",
            "Job outcomes by queue. outcome is one of: success, retried, dlq, failed.",
            &["queue", "outcome"]
        )
        .expect("register skill_pool_queue_jobs_total")
    })
}

/// Gauge handle for `skill_pool_queue_depth{queue=<name>}`.
pub fn queue_depth(queue: &str) -> IntGauge {
    queue_depth_vec().with_label_values(&[queue])
}

/// Gauge handle for `skill_pool_queue_dlq_depth{queue=<name>}`.
pub fn queue_dlq_depth(queue: &str) -> IntGauge {
    queue_dlq_depth_vec().with_label_values(&[queue])
}

/// Counter handle for `skill_pool_queue_jobs_total{queue,outcome}`.
/// `outcome` must be one of `success`, `retried`, `dlq`, `failed`.
pub fn queue_jobs_total(queue: &str, outcome: &str) -> IntCounter {
    queue_jobs_total_vec().with_label_values(&[queue, outcome])
}

// ---------------------------------------------------------------------------
// Tower middleware — observes every request that passes through the router
// ---------------------------------------------------------------------------

/// Axum middleware that records request count, duration, and in-flight gauge.
///
/// Wire it with `axum::middleware::from_fn_with_state` so it has access to
/// `AppState` and can sample the DB pool size alongside each request.
pub async fn track(State(state): State<AppState>, req: Request<Body>, next: Next) -> Response {
    // Capture matched route pattern (e.g. `/v1/skills/:slug`) so the
    // cardinality of the `path` label stays bounded even with UUID segments.
    let path = req
        .extensions()
        .get::<MatchedPath>()
        .map(|mp| mp.as_str().to_owned())
        .unwrap_or_else(|| req.uri().path().to_owned());

    let method = req.method().as_str().to_owned();

    http_requests_in_flight().inc();
    let start = Instant::now();

    let response = next.run(req).await;

    let elapsed = start.elapsed().as_secs_f64();
    let status = response.status().as_u16().to_string();

    http_requests_in_flight().dec();
    db_pool_size().set(state.db().size() as i64);

    let labels: &[&str] = &[method.as_str(), path.as_str(), status.as_str()];
    http_requests_total().with_label_values(labels).inc();
    http_request_duration_seconds()
        .with_label_values(labels)
        .observe(elapsed);

    response
}

// ---------------------------------------------------------------------------
// GET /metrics handler
// ---------------------------------------------------------------------------

/// Render the default Prometheus registry in text exposition format 0.0.4.
pub async fn handler() -> impl IntoResponse {
    let encoder = TextEncoder::new();
    let families = prometheus::gather();
    let mut buf = Vec::with_capacity(4096);
    encoder.encode(&families, &mut buf).expect("encode metrics");

    (
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4",
        )],
        buf,
    )
}
