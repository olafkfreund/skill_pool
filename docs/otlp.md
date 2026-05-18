# OTLP Distributed Tracing

Skill-pool-server ships an opt-in OpenTelemetry (OTLP) exporter. When enabled
it exports spans to any OTLP-compatible collector (Grafana Tempo, Honeycomb,
Datadog APM via the OTel collector, Jaeger) while keeping the existing JSON
log stream on stderr intact.

## Enabling

Build with the `otlp` Cargo feature:

```sh
cargo build -p skill-pool-server --features otlp
```

The default build (`--no-default-features` or plain `cargo build`) does NOT
compile any OpenTelemetry dependencies.

## Environment variables

| Variable | Default | Required | Description |
|---|---|---|---|
| `OTEL_EXPORTER_OTLP_ENDPOINT` | `http://localhost:4318` | No | HTTP base URL of the OTLP collector. Spans are POSTed to `{endpoint}/v1/traces`. |
| `OTEL_SERVICE_NAME` | `skill-pool-server` | No | Service name attached to every span. |

Standard OpenTelemetry SDK env vars (`OTEL_RESOURCE_ATTRIBUTES`,
`OTEL_TRACES_SAMPLER`, …) are not yet plumbed; use the two above for basic
identification.

## Transport

The exporter uses **HTTP/protobuf** (`http-proto`) over **rustls** — no
OpenSSL dependency is introduced. The default port for HTTP OTLP is **4318**
(gRPC is 4317, but gRPC requires tonic/OpenSSL; we intentionally avoid that).

## Collector quick-start

### Grafana Tempo (docker compose)

```yaml
services:
  tempo:
    image: grafana/tempo:latest
    command: ["-config.file=/etc/tempo.yaml"]
    ports:
      - "4318:4318"   # OTLP HTTP
      - "3200:3200"   # Tempo query API (for Grafana)
```

Set `OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318` and the server will
export traces to Tempo automatically.

### Jaeger (docker)

```sh
docker run -d --name jaeger \
  -p 4318:4318 \
  -p 16686:16686 \
  jaegertracing/jaeger:2
```

Jaeger 2.x accepts OTLP HTTP on port 4318 directly. Browse traces at
`http://localhost:16686`.

## Crate versions

| Crate | Version |
|---|---|
| `opentelemetry` | 0.31 |
| `opentelemetry_sdk` | 0.31 |
| `opentelemetry-otlp` | 0.31 |
| `tracing-opentelemetry` | 0.32 |

`tracing-opentelemetry 0.32.x` targets `opentelemetry 0.31.x`; all four
crates share the same opentelemetry major version at build time.
