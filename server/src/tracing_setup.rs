//! Tracing initialisation and per-request tenant span middleware.
//!
//! ## Log format
//!
//! Controlled by the `RUST_LOG_FORMAT` environment variable:
//!
//! * `pretty` (or any unrecognised value) → human-readable `tracing_subscriber::fmt`
//! * unset / `json` → JSON line-delimited output suitable for Loki/Splunk/CloudWatch
//!
//! `RUST_LOG` (or the `RUST_LOG` default `"warn,skill_pool=info"`) is honoured in
//! both modes via `EnvFilter`.
//!
//! ## Per-tenant span
//!
//! `tenant_span_layer` is an `axum::middleware::from_fn`-compatible async fn.
//! It opens an `info_span!("request", …)` that carries:
//!
//! * `tenant.slug` — extracted from the `X-Skill-Pool-Tenant` header or
//!   the leading subdomain of `Host`, exactly like [`crate::tenant::slug_from_request`]
//!   but without the DB round-trip.
//! * `http.method`
//! * `http.path`
//!
//! The span is entered before the handler runs and closed automatically when
//! the response future resolves. The existing `tower_http::trace::TraceLayer`
//! remains in place; this middleware is layered *before* it (outer) so that
//! `TraceLayer`'s own spans are children of the tenant span.

use axum::body::Body;
use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use tracing::Instrument;
use tracing_subscriber::EnvFilter;

// ---------------------------------------------------------------------------
// Init
// ---------------------------------------------------------------------------

/// Initialise the global tracing subscriber.
///
/// Call once at process start, before any `tracing::*` calls.
///
/// Format is selected from `RUST_LOG_FORMAT`:
/// - `"pretty"` → human-friendly text
/// - anything else (or unset) → JSON lines
pub fn init() {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,skill_pool=info"));

    let pretty = std::env::var("RUST_LOG_FORMAT")
        .unwrap_or_default()
        .to_lowercase()
        == "pretty";

    // Build one `Registry` so the OTel layer (feature-gated) can join the same
    // subscriber as the fmt layer. `Box<dyn Layer<_> + Send + Sync>` keeps the
    // type uniform across the pretty/json branches.
    let fmt_layer: Box<dyn tracing_subscriber::Layer<_> + Send + Sync> = if pretty {
        Box::new(tracing_subscriber::fmt::layer())
    } else {
        Box::new(tracing_subscriber::fmt::layer().json())
    };

    let registry = tracing_subscriber::registry().with(filter).with(fmt_layer);

    #[cfg(feature = "otlp")]
    {
        registry.with(crate::telemetry::otel_layer()).init();
    }
    #[cfg(not(feature = "otlp"))]
    {
        registry.init();
    }
}

// ---------------------------------------------------------------------------
// Per-tenant request span middleware
// ---------------------------------------------------------------------------

/// Axum middleware that wraps every request in a `tracing` span tagged with
/// tenant, method, and path.
///
/// Wire it with [`axum::middleware::from_fn`] *before* `TraceLayer` in the
/// layer stack so that the HTTP trace events appear as children of this span.
pub async fn tenant_span_layer(req: Request<Body>, next: Next) -> Response {
    let method = req.method().as_str().to_owned();
    let path = req.uri().path().to_owned();
    let tenant_slug = extract_slug(req.headers());

    let span = tracing::info_span!(
        "request",
        "tenant.slug" = %tenant_slug,
        "http.method" = %method,
        "http.path" = %path,
    );

    next.run(req).instrument(span).await
}

/// Extract the tenant slug from headers without hitting the database.
///
/// Priority:
/// 1. `X-Skill-Pool-Tenant` header (dev / CI override)
/// 2. Leading subdomain of `Host`
/// 3. `"-"` sentinel when neither is present (healthz, /metrics, etc.)
fn extract_slug(headers: &axum::http::HeaderMap) -> String {
    // Explicit header takes priority.
    if let Some(val) = headers.get("x-skill-pool-tenant") {
        if let Ok(s) = val.to_str() {
            let s = s.trim().to_lowercase();
            if !s.is_empty() {
                return s;
            }
        }
    }

    // Fall back to Host subdomain.
    if let Some(host_val) = headers.get("host") {
        if let Ok(host) = host_val.to_str() {
            let host_no_port = host.split(':').next().unwrap_or(host);
            if let Some(label) = host_no_port.split('.').next() {
                if !label.is_empty() && label != "www" {
                    return label.to_lowercase();
                }
            }
        }
    }

    "-".to_owned()
}
