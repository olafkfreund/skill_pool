# Ops kit

Starter Prometheus + Grafana artifacts for the skill-pool registry server.
The server already emits everything these files consume — see
`server/src/metrics.rs` and `/metrics` on a running instance.

## What's here

```
ops/
├── grafana/
│   └── skill-pool.json              # Importable Grafana dashboard
└── alerts/
    └── skill-pool.rules.yaml        # Prometheus alert rules
```

## Metrics available

| Metric                              | Type      | Labels                  |
|-------------------------------------|-----------|-------------------------|
| `http_requests_total`               | counter   | `method`, `path`, `status` |
| `http_request_duration_seconds`     | histogram | `method`, `path`, `status` |
| `http_requests_in_flight`           | gauge     | —                       |
| `db_pool_size`                      | gauge     | —                       |

There is **no `tenant` label on metrics** — tenant attribution lives on
tracing spans (`tenant.slug`, `tenant.id` — see
`server/src/tracing_setup.rs`). For per-tenant breakdowns, pivot to
Tempo/Jaeger via the OTLP exporter (`--features otlp`, env
`OTEL_EXPORTER_OTLP_ENDPOINT`) or to your log backend (Loki/Splunk/CloudWatch)
which receives the same attribute on every request log line.

## Prometheus scrape config

```yaml
scrape_configs:
  - job_name: skill-pool
    metrics_path: /metrics
    static_configs:
      - targets: ['skill-pool.internal:8080']
```

`/metrics` requires no auth. Restrict access at the network layer
(reverse proxy ACL or private network).

## Loading the alert rules

```yaml
# prometheus.yml
rule_files:
  - /etc/prometheus/rules/skill-pool.rules.yaml
```

Validate before loading:

```bash
promtool check rules ops/alerts/skill-pool.rules.yaml
```

The `SkillPoolHealthzDBDown` rule depends on a `blackbox_exporter` probe
of `/v1/healthz` that parses the JSON response into a
`probe_http_content_status_db` gauge. If you don't run blackbox-exporter
that way, delete the `skill-pool.healthz` group — the other four rules
(error rate / latency / DB pool / no traffic) tell you everything the
healthz probe would.

Example blackbox config:

```yaml
modules:
  skill_pool_healthz:
    prober: http
    http:
      method: GET
      valid_status_codes: [200]
      fail_if_body_json_matches:
        - path: ".deps.db.status"
          comparison: "not_equal"
          value: "up"
```

## Importing the dashboard

Grafana **Dashboards → New → Import**, paste the contents of
`grafana/skill-pool.json`, and pick your Prometheus datasource when
prompted. The dashboard ships with one template variable:

- **Route** — multi-select of `path` label values. `All` is the default.

## Panel reference

| Row | Panel                              | Source metric                                |
|-----|------------------------------------|----------------------------------------------|
| 1   | Request rate (stat)                | `rate(http_requests_total[5m])`              |
| 1   | Error rate 5xx (stat)              | `http_requests_total{status=~"5.."}`         |
| 1   | p95 latency (stat)                 | `http_request_duration_seconds_bucket`       |
| 1   | In-flight requests (stat)          | `http_requests_in_flight`                    |
| 2   | Request rate by status class       | `http_requests_total` (re-labelled to `1xx..5xx`) |
| 2   | Top routes by request rate         | `topk(10, sum by (path) (...))`              |
| 3   | Latency p50/p95/p99                | `histogram_quantile` over buckets             |
| 3   | Latency heatmap                    | per-bucket rate                              |
| 4   | Errors by path (4xx + 5xx)         | `http_requests_total{status=~"4..|5.."}`     |
| 5   | DB pool size                       | `db_pool_size`                               |
| 5   | In-flight (timeseries)             | `http_requests_in_flight`                    |

## Alerts at a glance

| Alert                          | Severity | Trigger                                        |
|--------------------------------|----------|------------------------------------------------|
| `SkillPoolHighErrorRate`       | critical | 5xx rate > 5 % of traffic for 5 min            |
| `SkillPoolHighLatency`         | warning  | p95 > 1 s for 10 min                            |
| `SkillPoolDBPoolExhausted`     | critical | `db_pool_size ≤ 1` while requests are arriving |
| `SkillPoolNoTraffic`           | warning  | zero requests for 10 min                        |
| `SkillPoolHealthzDBDown`       | critical | healthz probe reports `deps.db.status=down`     |

Tune `for:` durations and thresholds to your traffic profile before
shipping to production — the defaults are conservative for a small team
deployment and may be too sensitive in a high-throughput environment or
too lax in a low-throughput one.
