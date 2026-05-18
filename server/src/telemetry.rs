//! Telemetry initialisation.
//!
//! Always sets up a `tracing_subscriber::fmt` JSON layer that writes to
//! stderr (so the existing log format is preserved).
//!
//! When the `otlp` Cargo feature is enabled, an additional
//! `tracing_opentelemetry` layer is stacked on top, exporting spans to an
//! OTLP-compatible collector via HTTP/protobuf (no OpenSSL required — uses
//! rustls).  The fmt layer continues to operate normally — both sinks receive
//! every event.
//!
//! # Environment variables (otlp feature only)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318` | HTTP base URL of the collector |
//! | `OTEL_SERVICE_NAME` | `skill-pool-server` | Service name reported in spans |

use anyhow::Result;

/// Initialise the global tracing subscriber.
///
/// Call exactly once, before any `tracing` macros are used.
pub fn init() -> Result<()> {
    use tracing_subscriber::EnvFilter;

    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let fmt_layer = tracing_subscriber::fmt::layer().json();

    #[cfg(not(feature = "otlp"))]
    {
        use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
        tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .init();
    }

    #[cfg(feature = "otlp")]
    {
        use opentelemetry::trace::TracerProvider as _;
        use opentelemetry_otlp::WithExportConfig;
        use opentelemetry_sdk::trace::SdkTracerProvider;
        use tracing_subscriber::prelude::*;

        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318".to_owned());

        let service_name = std::env::var("OTEL_SERVICE_NAME")
            .unwrap_or_else(|_| "skill-pool-server".to_owned());

        let exporter = opentelemetry_otlp::SpanExporter::builder()
            .with_http()
            .with_endpoint(format!("{endpoint}/v1/traces"))
            .build()
            .map_err(|e| anyhow::anyhow!("OTLP span exporter build failed: {e}"))?;

        let resource = opentelemetry_sdk::Resource::builder_empty()
            .with_service_name(service_name.clone())
            .build();

        let provider = SdkTracerProvider::builder()
            .with_resource(resource)
            .with_batch_exporter(exporter)
            .build();

        // Store the provider so shutdown() can flush it later.
        TRACER_PROVIDER
            .set(provider.clone())
            .expect("telemetry::init must be called exactly once");

        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        opentelemetry::global::set_tracer_provider(provider.clone());

        let tracer = provider.tracer("skill-pool-server");
        let otel_layer = tracing_opentelemetry::layer().with_tracer(tracer);

        let subscriber = tracing_subscriber::registry()
            .with(env_filter)
            .with(fmt_layer)
            .with(otel_layer);
        tracing::subscriber::set_global_default(subscriber)
            .map_err(|e| anyhow::anyhow!("set_global_default failed: {e}"))?;

        tracing::info!(
            endpoint = %endpoint,
            service.name = %service_name,
            "OTLP exporter enabled (http-proto/rustls)"
        );
    }

    Ok(())
}

#[cfg(feature = "otlp")]
static TRACER_PROVIDER: std::sync::OnceLock<opentelemetry_sdk::trace::SdkTracerProvider> =
    std::sync::OnceLock::new();

/// Flush and shut down the OTLP pipeline on graceful shutdown.
///
/// In default (non-otlp) builds this is a no-op.
pub fn shutdown() {
    #[cfg(feature = "otlp")]
    if let Some(provider) = TRACER_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            eprintln!("OTLP tracer provider shutdown error: {e}");
        }
    }
}
