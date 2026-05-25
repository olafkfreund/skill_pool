//! OpenTelemetry (OTLP) pipeline.
//!
//! Default builds: every public function is a no-op. The crate adds nothing
//! and pulls no extra deps.
//!
//! `--features otlp` builds: [`init`] configures the OTel SDK and stashes the
//! [`SdkTracerProvider`] for later flushing; [`shutdown`] drains it. The
//! `tracing-opentelemetry` layer is added to the global subscriber by
//! [`crate::tracing_setup::init`] via [`otel_layer`].
//!
//! # Environment variables (otlp feature only)
//!
//! | Variable | Default | Description |
//! |---|---|---|
//! | `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318` | HTTP base URL of the collector |
//! | `OTEL_SERVICE_NAME` | `skill-pool-server` | Service name reported in spans |

use anyhow::Result;

#[cfg(feature = "otlp")]
static TRACER_PROVIDER: std::sync::OnceLock<opentelemetry_sdk::trace::SdkTracerProvider> =
    std::sync::OnceLock::new();

/// Configure the OTLP SDK pipeline.
///
/// Default build: no-op. `--features otlp`: build the span exporter, propagator,
/// and tracer provider, and stash the provider for [`shutdown`].
///
/// Subscriber wiring happens in [`crate::tracing_setup::init`] — this function
/// does **not** touch the global subscriber.
pub fn init() -> Result<()> {
    #[cfg(feature = "otlp")]
    {
        use opentelemetry_otlp::WithExportConfig;
        use opentelemetry_sdk::trace::SdkTracerProvider;

        let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
            .unwrap_or_else(|_| "http://localhost:4318".to_owned());

        let service_name =
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "skill-pool-server".to_owned());

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

        TRACER_PROVIDER
            .set(provider.clone())
            .map_err(|_| anyhow::anyhow!("telemetry::init must be called exactly once"))?;

        opentelemetry::global::set_text_map_propagator(
            opentelemetry_sdk::propagation::TraceContextPropagator::new(),
        );
        opentelemetry::global::set_tracer_provider(provider);

        tracing::info!(
            endpoint = %endpoint,
            service.name = %service_name,
            "OTLP exporter enabled (http-proto/rustls)"
        );
    }
    Ok(())
}

/// `tracing-opentelemetry` layer for chaining onto the global Registry.
///
/// Default build: this function is absent. `--features otlp`: returns a
/// [`tracing_opentelemetry::OpenTelemetryLayer`] keyed on the provider stashed
/// by [`init`]. Call [`init`] before this.
#[cfg(feature = "otlp")]
pub fn otel_layer<S>(
) -> tracing_opentelemetry::OpenTelemetryLayer<S, opentelemetry_sdk::trace::Tracer>
where
    S: tracing::Subscriber + for<'span> tracing_subscriber::registry::LookupSpan<'span>,
{
    use opentelemetry::trace::TracerProvider as _;
    let provider = TRACER_PROVIDER
        .get()
        .expect("telemetry::init must be called before otel_layer");
    let tracer = provider.tracer("skill-pool-server");
    tracing_opentelemetry::layer().with_tracer(tracer)
}

/// Flush and shut down the OTLP pipeline on graceful shutdown.
///
/// Default build: no-op.
pub fn shutdown() {
    #[cfg(feature = "otlp")]
    if let Some(provider) = TRACER_PROVIDER.get() {
        if let Err(e) = provider.shutdown() {
            eprintln!("OTLP tracer provider shutdown error: {e}");
        }
    }
}
