# skill-pool incident runbook

## 1. Using this runbook

This document is for the engineer on call. It is organised by alert name: when a page fires, jump to the matching subsection in *Incident playbooks*. The severity convention matches the `severity:` label on the Prometheus rules in `ops/alerts/skill-pool.rules.yaml` — `critical` means page someone, `warning` means a ticket. The runbook assumes the on-caller has either a shell on the server (single-node / systemd deploys) or `kubectl` against the cluster, or both; commands are given for both where they differ.

## 2. Triage flow

Before opening a specific playbook, do these five things in order. They take under two minutes and rule out the most common false alarms.

1. Hit healthz and read `deps`:

   ```bash
   curl -s https://<host>/v1/healthz | jq
   ```

   Each entry under `deps` has `status: up|down|off`. `off` means the component is intentionally disabled (e.g. `embedder` when `--features fastembed` is not built in) — not an incident.

2. Tail the service log:

   ```bash
   # systemd
   journalctl -u skill-pool-server -n 200 --no-pager
   # kubernetes
   kubectl logs -n <ns> deploy/skill-pool-server --tail=200
   ```

3. Open the Grafana dashboard `skill-pool — server overview` and glance at the top row: `Request rate`, `Error rate (5xx)`, `p95 latency`, `In-flight requests`.

4. Check `db_pool_size` on the dashboard (panel: `DB pool size`). Sustained values at or below 1 with active traffic are a critical signal.

5. Check for a recent deploy. On the deploy host or in CI:

   ```bash
   git log --oneline -10 origin/main
   ```

   If a deploy went out in the last hour, suspect it first.

## 3. Incident playbooks

### `SkillPoolHighErrorRate`

**Severity:** critical.

**What it means:** more than 5% of HTTP responses have been 5xx for 5 minutes (rule expression in `ops/alerts/skill-pool.rules.yaml`).

**First 60 seconds:**

```bash
curl -s https://<host>/v1/healthz | jq '.deps'
```

If `deps.db.status=="down"` jump to `SkillPoolHealthzDBDown`. If `deps.storage.status=="down"` jump to common cause 2 below.

**Investigation queries:**

```promql
# Which routes are throwing 5xx
topk(10, sum by (path) (rate(http_requests_total{status=~"5.."}[5m])))
```

```promql
# Which status codes specifically
sum by (status) (rate(http_requests_total{status=~"5.."}[5m]))
```

```bash
# Errors in the service log, last 5 min
journalctl -u skill-pool-server --since "5 min ago" -p err --no-pager
```

```bash
# When RUST_LOG_FORMAT=json — filter only error-level entries
journalctl -u skill-pool-server --since "5 min ago" -o cat \
  | jq -c 'select(.level=="ERROR")'
```

**Common causes:**

1. **Bad deploy.** Confirm with `git log --oneline -10` on the deploy branch. Fix: roll back (systemd: install the previous binary and `systemctl restart skill-pool-server`; k8s: `kubectl rollout undo deployment/skill-pool-server`).
2. **Storage backend unreachable.** Confirm with `curl -s /v1/healthz | jq '.deps.storage'` — looks for `status:"down"` and an `error` string from opendal. Fix: for `fs://` check the `ReadWritePaths` directory mount and disk space (`df -h`); for `s3://` check IAM/network from the host.
3. **DB connection storm after Postgres restart.** Confirm with `journalctl -u skill-pool-server | grep -i 'pool\|sqlx'`. Fix: restart the server to drop stale connections (`systemctl restart skill-pool-server`).
4. **Request body limit exceeded on `/v1/skills` POST.** The limit is 5 MiB plus 64 KiB of metadata; oversize uploads return 413, not 5xx, but a misconfigured ingress in front of skill-pool can rewrite this to 502. Confirm with `topk(10, sum by (path,status) (rate(http_requests_total[5m])))`.
5. **Panic loop.** Confirm with `journalctl -u skill-pool-server | grep -i panic`. Fix: rollback; capture the stack for a bug report.

**Mitigations:** roll back the last deploy; failing that, route traffic away at the load balancer until the cause is found.

---

### `SkillPoolHighLatency`

**Severity:** warning.

**What it means:** p95 of `http_request_duration_seconds_bucket` has been above 1 second for 10 minutes.

**First 60 seconds:**

```promql
# Which route is slow
topk(5,
  histogram_quantile(0.95,
    sum by (le, path) (rate(http_request_duration_seconds_bucket[5m]))
  )
)
```

**Investigation queries:**

```promql
# p50 vs p95 vs p99 — is this a tail or a shift?
histogram_quantile(0.50, sum by (le) (rate(http_request_duration_seconds_bucket[5m])))
histogram_quantile(0.95, sum by (le) (rate(http_request_duration_seconds_bucket[5m])))
histogram_quantile(0.99, sum by (le) (rate(http_request_duration_seconds_bucket[5m])))
```

```promql
# Concurrent requests — saturation indicator
http_requests_in_flight
```

```bash
# Postgres slow-query log (>500ms, if log_min_duration_statement is set)
sudo -u postgres tail -200 /var/log/postgresql/postgresql-*.log | grep duration:
```

**Common causes:**

1. **Missing pgvector index** when running `/v1/skills?semantic=...`. Confirm: route `path="/v1/skills"` dominates `topk` above. Fix: rebuild the embeddings index — see `docs/deploy/single-node.md` for the `CREATE INDEX ... USING ivfflat` recipe.
2. **DB pool saturated.** Confirm: `db_pool_size` is at the cap (currently hardcoded 20 in `server/src/state.rs`) and `http_requests_in_flight` is rising. Fix: short-term restart; long-term raise the pool size.
3. **Cold embedder.** First request after restart with `--features fastembed` downloads a model. Confirm: spike at startup, recovers within ~60 s. No action needed unless persistent.
4. **Noisy neighbour on the host.** Confirm: `top`, `iostat -x 1` on the box; CPU steal time non-zero on a VM. Fix: move workload to a less contended host.
5. **Bundle uploads.** `POST /v1/skills` with a 5 MiB body is bound by storage write speed. Confirm: panel `Latency by route` shows `/v1/skills` POST. Mitigation: rate-limit upstream.

**Mitigations:** none acute — this is a warning, not a page. If user-impacting, declare an incident and proceed as for `SkillPoolHighErrorRate`.

---

### `SkillPoolDBPoolExhausted`

**Severity:** critical.

**What it means:** `db_pool_size` has been at or below 1 while requests are arriving (the gauge measures sqlx's current pool size, sampled on every request in `server/src/metrics.rs`).

**First 60 seconds:**

```bash
sudo -u postgres psql -c "
  SELECT pid, state, wait_event, now()-query_start AS dur, left(query,80) AS q
  FROM pg_stat_activity
  WHERE datname='skillpool' AND state <> 'idle'
  ORDER BY dur DESC
  LIMIT 10;
"
```

**Investigation queries:**

```promql
# Confirm the pool is pinned
db_pool_size
```

```promql
# Is traffic spiking, or are connections being held?
sum(rate(http_requests_total[1m]))
```

```bash
# Long transactions / locks
sudo -u postgres psql -c "
  SELECT relation::regclass, mode, granted, pid
  FROM pg_locks WHERE NOT granted;
"
```

**Common causes:**

1. **Long-running query holding a connection.** Confirm: `pg_stat_activity` shows a query with `dur > 30s`. Fix: `SELECT pg_cancel_backend(<pid>);` for the offending pid, then add an index for that query.
2. **Pool max-connections too low for traffic.** The pool cap is hardcoded at 20 (`PgPoolOptions::new().max_connections(20)` in `server/src/state.rs`). Fix: raise it in source and redeploy; there is no live-config knob for this today. [TODO: verify `SKILL_POOL_DB_POOL_SIZE` — referenced in alert annotations but not present in `server/src/config.rs` as of this revision.]
3. **Postgres restarted, sqlx still holds dead handles.** Confirm: errors like `connection closed` in the log. Fix: restart skill-pool-server.
4. **Deadlock.** Confirm: `pg_locks WHERE NOT granted` shows two PIDs blocking each other. Fix: cancel both; investigate the offending route.
5. **Connection leak in a new code path.** Confirm: pool stays low even when traffic drops. Fix: rollback the most recent deploy.

**Mitigations:** restart the server (`systemctl restart skill-pool-server` or `kubectl rollout restart deployment/skill-pool-server`) — this drops all pool connections and forces a clean reconnect. Re-fires within minutes if the root cause is unaddressed.

---

### `SkillPoolNoTraffic`

**Severity:** warning.

**What it means:** Prometheus is still scraping `/metrics` (otherwise this alert could not fire) but no requests have been counted for 10 minutes.

**First 60 seconds:**

```bash
curl -s -o /dev/null -w "%{http_code}\n" https://<public-host>/v1/healthz
```

If this returns 200, the server is reachable from your laptop — the issue is upstream of clients (likely DNS / LB / CDN), not skill-pool. If it returns a non-200 or hangs, work back through the path: LB health, ingress, DNS.

**Investigation queries:**

```promql
# Did the rate truly go to zero, or is the scrape stale?
sum(rate(http_requests_total[10m]))
```

```promql
# Confirm Prometheus is still scraping
up{job="skill-pool"}
```

```bash
# From inside the cluster / on the host — is the server itself receiving?
curl -s http://127.0.0.1:8080/v1/healthz
```

**Common causes:**

1. **Load balancer / ingress dropped the backend.** Confirm: external curl fails, internal curl works. Fix: re-add backend, check LB health-check path (should be `/v1/healthz`, which returns 200 even when `deps.db.status=="down"` by design).
2. **DNS change.** Confirm: `dig <host>` returns wrong IP. Fix: revert DNS.
3. **Genuinely idle deployment** (small tenant, weekend, post-promotion window). Confirm: this is expected. Fix: silence the alert for known-quiet windows.
4. **TLS certificate expired at the edge.** Confirm: external curl shows TLS handshake error. Fix: renew / rotate the cert at the LB.

**Mitigations:** if external traffic cannot reach the box, re-route at DNS or LB level.

---

### `SkillPoolHealthzDBDown`

**Severity:** critical.

**What it means:** the blackbox probe of `/v1/healthz` has parsed `deps.db.status=="down"` for 2 minutes. The server itself answers (the healthz handler returns HTTP 200 even on DB failure — see `server/src/routes/health.rs`), but the `SELECT 1` it issues against Postgres is failing.

**First 60 seconds:**

```bash
curl -s https://<host>/v1/healthz | jq '.deps.db'
```

The `error` field carries the sqlx error string verbatim — read it. Common shapes: `connection refused`, `password authentication failed`, `database "skillpool" does not exist`, `timeout`.

**Investigation queries:**

```bash
# Is Postgres up at all?
sudo systemctl status postgresql            # single-node
kubectl get pods -n <ns> -l app=postgres    # k8s
```

```bash
# Can the server host reach Postgres?
nc -zv <db-host> 5432
```

```bash
# Postgres own log
sudo -u postgres tail -100 /var/log/postgresql/postgresql-*.log
```

**Common causes:**

1. **Postgres process down.** Confirm: `systemctl status postgresql` shows inactive. Fix: `systemctl start postgresql`; investigate why it died.
2. **Disk full on Postgres host.** Confirm: `df -h` on the DB host shows 100% on the data volume. Fix: add space, vacuum, restart Postgres.
3. **Credentials drifted.** Confirm: error string `password authentication failed`. Fix: align `SKILL_POOL_DATABASE_URL` (set via `EnvironmentFile=/etc/skill-pool/skill-pool-server.env`) with the actual Postgres role.
4. **Network partition / security group change.** Confirm: `nc -zv` from the server host fails. Fix: restore the network path.
5. **`max_connections` reached on Postgres.** Confirm: error string `too many connections` or `remaining connection slots are reserved`. Fix: raise `max_connections` in `postgresql.conf` and restart Postgres.

**Mitigations:** the server keeps responding HTTP 200 on `/v1/healthz` by design so the load balancer does not pull it; reads against cached state still work. Restoring Postgres is the only real fix.

## 4. Common operational tasks

All commands run as the `skillpool` user on the server host unless noted.

### Promote a SAML or OIDC config

```bash
# OIDC
skill-pool-server admin sso-set \
  --tenant acme \
  --issuer https://acme.okta.com/oauth2/default \
  --client-id <id> \
  --client-secret <secret> \
  --default-role viewer

# SAML
skill-pool-server admin saml-set \
  --tenant acme \
  --idp-entity-id https://acme.okta.com/exk... \
  --idp-sso-url https://acme.okta.com/app/.../sso/saml \
  --idp-cert-path /etc/skill-pool/idp.pem \
  --default-role viewer
```

### Create a tenant and mint the first token

```bash
skill-pool-server admin tenant-create --slug acme --name "Acme Inc" --plan team
skill-pool-server admin token-create --tenant acme --name bootstrap \
  --scope "skills:read skills:publish"
```

The raw token is printed once. Capture it immediately.

### Rotate an API token

1. Mint a replacement with the same scope:

   ```bash
   skill-pool-server admin token-create --tenant acme --name ci-rotated \
     --scope "skills:read skills:publish"
   ```

2. Distribute the new token to the consumer.
3. Revoke the old one. [TODO: verify the exact `admin token-revoke` command — not present in `server/src/main.rs` Cmd enum as of this revision.]
4. Watch `http_requests_total{status="401"}` for an hour to make sure no client was missed.

### Backfill embeddings after enabling `--features fastembed`

```bash
# Dry run first
skill-pool-server admin backfill-embeddings --limit 500 --dry-run

# Then for real, per tenant
skill-pool-server admin backfill-embeddings --tenant acme --limit 500
```

Skill rows with NULL `description_embedding` are filled in; rows the embedder declines are skipped silently.

### Force a graceful drain

```bash
# systemd — sends SIGTERM, waits TimeoutStopSec=30s
systemctl stop skill-pool-server
```

The handler in `server/src/main.rs` selects on Ctrl+C and SIGTERM and lets in-flight requests finish. If `systemctl stop` hangs past 30 seconds, systemd sends SIGKILL (default `KillMode`); inspect for stuck downloads or a runaway DB query:

```bash
ss -tnp | grep :8080
sudo -u postgres psql -c "SELECT pid, state, query FROM pg_stat_activity WHERE datname='skillpool';"
```

### Rolling restart in Kubernetes

```bash
kubectl rollout restart deployment/skill-pool-server -n <ns>
kubectl rollout status  deployment/skill-pool-server -n <ns>
```

The pod's `terminationGracePeriodSeconds` should be ≥30 s to match the in-process drain budget.

### Read logs filtered by tenant

Only works when `RUST_LOG_FORMAT=json` is set (the tenant slug is on the `request` span, not a free-text field).

```bash
journalctl -u skill-pool-server -o cat \
  | jq -c 'select(.span.["tenant.slug"]=="acme")'
```

For the `pretty` formatter the tenant appears in span context near the start of each line; grep `tenant.slug=acme`.

### Inspect OTLP traces (Tempo / Jaeger)

Available only when the server is built with `--features otlp` and `OTEL_EXPORTER_OTLP_ENDPOINT` is set (see `server/src/telemetry.rs`). Default service name is `skill-pool-server`. Sample TraceQL for Tempo:

```
{ service.name = "skill-pool-server" && tenant.slug = "acme" }
```

The `tenant.slug`, `http.method`, and `http.path` attributes are set on the root `request` span by `tenant_span_layer` in `server/src/tracing_setup.rs`.

## 5. Disaster recovery

### Postgres lost, bundles survive

The bundle objects on the storage backend (`SKILL_POOL_STORAGE_URI`, default `fs:///var/lib/skill-pool/storage`) are content-addressed by SHA-256 and are the source of truth for `SKILL.md` content.

1. Provision a fresh Postgres and create the role and database.
2. Restore the most recent dump:

   ```bash
   pg_restore --clean --if-exists -d skillpool /backup/skillpool-YYYYMMDD.dump
   ```

3. Bring skill-pool-server back up; migrations are idempotent and will fast-path.
4. For any catalog row whose recorded `sha256` does not match the on-disk object, re-upload from CI; the publish endpoint enforces the checksum.

### Bundle storage lost, Postgres survives

The catalog DB still knows every slug, version, and SHA-256.

1. Replace the storage backend (new disk / new bucket).
2. Re-publish every skill from the Git mirror or CI. The server rejects uploads whose body SHA-256 does not match the row already in `skills`; a clean re-publish reconciles.
3. Inspect `Errors by path (4xx + 5xx)` after the re-publish window — 404s on `GET /v1/skills/{slug}/bundle.tar.gz` indicate skills not yet re-uploaded.

### Both lost

Skill content is recoverable from the Git source-of-truth mirror that produced the bundles. Tenant rows, API tokens, theme overrides, SSO configs, and curator-notification settings are not in Git and must be re-created with the `admin` subcommands. Capture this list before you start so the restore is auditable.

## 6. Escalation

Wake the next person up when any of the following is true.

| Condition | Page who | Why |
|---|---|---|
| Hardware or cloud-provider outage (host unreachable, region degraded, S3/RDS degraded) | Platform / infra on-call | skill-pool cannot self-heal through this; the fix is at the provider layer. |
| Suspected security incident: leaked token in logs, unexpected `admin` CLI invocations, suspected RCE in a published bundle | Security on-call | Tokens may need mass revocation; the bundle may need to be pulled from storage. |
| Data corruption suspected — `http_requests_total{status="5.."} > 1%` together with sqlx error logs mentioning `unique violation`, `foreign key constraint`, or `check constraint` | Database owner | A bad migration or a write path corrupting state needs eyes that have schema context. |
