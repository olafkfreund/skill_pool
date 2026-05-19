# Per-tenant rate limits

`skill-pool` enforces a per-tenant request budget at the HTTP edge. The
limiter sits between the existing `TraceLayer` and the Prometheus
`metrics::track` middleware in the router stack: it sees every API
request, attributes the cost to a single tenant, and returns `429 Too
Many Requests` when the configured cap is exceeded.

This document covers the design, the plan defaults, the admin recipe
for per-tenant overrides, the HTTP response shape, and the fail-open
behaviour during a Redis outage.

## What this protects against

Three classes of misbehaviour at the tenant boundary:

1. **Runaway scripts.** A developer's `xargs -P 100 curl …` loop fan-
   ning out a publish job, a buggy CI hook re-trying every second, an
   `inotify` integration that publishes on every keystroke. The
   limiter cuts the fan-out short before the catalog DB feels it.
2. **Resource abuse.** A tenant whose users decide skill-pool is a
   poor man's CDN and start hot-linking bundles. The 60-second cap
   caps that traffic at sustainable levels per plan.
3. **Single-tenant DoS.** A noisy tenant cannot blast through the
   per-process and per-DB capacity to the detriment of other tenants
   sharing the same node. The Redis counter is keyed by `tenant_id`,
   so quotas isolate cleanly.

This does **not** replace WAF / DDoS at the proxy edge. The limiter
runs in the application layer and assumes anything below it (TCP
SYN-flood, raw bandwidth saturation) is the operator's responsibility.

## Two windows: per-minute + per-second

Every tenant-attributable request increments **two** Redis counters:

* `rl:1m:<tenant_id>:<floor(now/60)>` — the 60-second window. Cap:
  the tenant's `rate_limit_rpm`.
* `rl:1s:<tenant_id>:<floor(now)>` — the 1-second window. Cap: the
  tenant's `rate_limit_burst`.

Both must pass for the request to proceed. The 60s window stops a
steady-state runaway script; the 1s window stops a single fan-out
spike (CI pipeline, `xargs -P 100 curl …`) from saturating the backend
even when the per-minute budget would have allowed it.

The algorithm is a **fixed-window counter**: keys are bucketed by
`floor(now / window)`, incremented with `INCR`, and the TTL is set
with `EXPIRE` (90s for the minute key, 5s for the second key — both
deliberately generous so a narrowly-missed expiry doesn't drop a
counter mid-window). The whole pair is shipped in one Redis pipeline
so the cost is a single round-trip per request.

Fixed windows have a known pathology: a client timing requests to land
just before a window boundary can momentarily double the cap for a
single second. v1 accepts this in exchange for radically simpler code.
Sliding-window via Lua or the `redis-cell` module is on the future-
work list.

## Plan defaults

Limits scale with plan tier; the values below are the baseline when
`tenants.rate_limit_rpm` and `tenants.rate_limit_burst` are both NULL.

| Plan         | RPM      | Burst (per-sec) | Sustained rps | Typical ceiling                                  |
| ------------ | -------- | --------------- | ------------- | ------------------------------------------------ |
| `team`       | 600      | 60              | 10            | Single small team, hand-driven publishes         |
| `business`   | 3 000    | 300             | 50            | Mid-size org with CI pipelines, MCP integrations |
| `enterprise` | 30 000   | 1 000           | 500           | Multi-region rollouts, large agentic workloads   |

The numbers are deliberately conservative — they exist to catch bugs,
not to throttle real customers. Operators with a real customer who
genuinely needs more should bump the per-tenant overrides (next
section) rather than the plan defaults.

## Per-tenant overrides

Two nullable columns on `tenants`:

```sql
rate_limit_rpm   INTEGER  -- override; NULL = plan default
rate_limit_burst INTEGER  -- override; NULL = plan default
```

Range constraints (DB-enforced):

* `rate_limit_rpm` ∈ (0, 100 000]
* `rate_limit_burst` ∈ (0, 10 000]

Either column can be set independently; an unset column inherits the
plan default. **Both** NULL = full plan default.

### Admin CLI

```bash
# Bump rpm only, leave burst on the plan default.
skill-pool-server admin tenant-rate-limits --slug acme --rpm 10000

# Or both at once, e.g. for a noisy enterprise migration.
skill-pool-server admin tenant-rate-limits --slug acme --rpm 50000 --burst 2000

# Burst-only — protect a tenant whose total volume is fine but
# whose CI fan-out is bursting too aggressively.
skill-pool-server admin tenant-rate-limits --slug acme --burst 500

# Revert to plan defaults (clear both columns).
skill-pool-server admin tenant-rate-limits --slug acme --clear
```

Clap enforces `--clear` vs `--rpm`/`--burst` mutual exclusion at the
CLI; `0` is rejected with a friendly message before hitting the DB
CHECK so operators don't see a raw sqlx error.

Changes take effect on the next request — there is no per-process
cache TTL in v1 because the SELECT on cold lookup is a primary-key
scan on a tiny table.

## HTTP response shape

### Successful (allowed) requests

The limiter attaches three response headers so well-behaved clients
can self-pace:

```
X-RateLimit-Limit: 3000
X-RateLimit-Remaining: 2997
X-RateLimit-Reset: 1719446400
```

* `X-RateLimit-Limit` — the tenant's effective RPM (the 60-second
  window cap, since that's the one most clients care about).
* `X-RateLimit-Remaining` — how many more requests fit in the current
  60-second window before the cap is reached.
* `X-RateLimit-Reset` — Unix timestamp at which the current 60s window
  rolls over.

### Throttled (denied) requests

```
HTTP/1.1 429 Too Many Requests
Retry-After: 37
X-RateLimit-Limit: 3000
X-RateLimit-Remaining: 0
X-RateLimit-Reset: 1719446437
Cache-Control: no-store
Content-Type: application/json

{
  "error": "rate_limit_exceeded",
  "message": "tenant rate limit exceeded; retry after the window resets",
  "retry_after_seconds": 37
}
```

* `Retry-After` is the standard HTTP header (seconds until retry).
* For a burst-cap breach `Retry-After` is at most `1` — clients
  retrying on a tiny backoff loop usually succeed on the next try.
* For an RPM-cap breach `Retry-After` points to the end of the
  current minute. Clients should switch to a longer backoff.

## Bypass list (no rate limiting)

The following paths are **unconditionally exempt** from rate limiting,
because they have no tenant context to attribute against or because
throttling them would break a critical flow:

| Path                                          | Reason                                                 |
| --------------------------------------------- | ------------------------------------------------------ |
| `/v1/healthz`                                 | Liveness probe; throttling would self-DoS the LB.      |
| `/metrics`                                    | Prometheus scrape; same reason.                        |
| `/v1/theme`, `/v1/theme/*`                    | Login-page branding; pre-auth, no tenant header.       |
| `/v1/og`                                      | Open-Graph card; we want social crawlers to fetch.     |
| `/v1/tenant/profile/banner`                   | CLI startup banner; subdomain-only resolution.         |
| `/v1/tenant/session-policy`                   | Login flow reads this before authenticating.           |
| `/v1/auth/oidc/*`, `/v1/auth/saml/*`          | SSO callbacks; pre-token.                              |
| `/v1/tenant/custom-domains/{host}/cert-ok`    | Caddy `on_demand_tls.ask` — called by the proxy.       |

Real API paths (`/v1/skills`, `/v1/drafts`, `/v1/mcp`, `/v1/tenant/*`
admin routes, SCIM …) are all rate-limited.

## Fail-open semantics

If Redis is unreachable (DNS failure, dial timeout, broken TCP
connection mid-pipeline), the limiter **fails open**: the request
proceeds without enforcement. We prefer availability to strict
enforcement during a cache outage — a Redis hiccup should not 500 the
whole API.

The trade-off is explicit: during a Redis outage, a misbehaving
tenant can shovel unlimited traffic. The mitigations are:

* Redis is in the same private network as the server. A network
  partition that knocks it out usually knocks out the whole site
  anyway.
* Every Redis failure logs at `WARN` with the failing tenant slug,
  so an operator can spot a sustained outage.
* The `/metrics` endpoint shows the per-tenant 429 rate dropping to
  zero — sudden cliff in the 429 rate next to a sustained RPS is a
  reliable indicator that the limiter is offline.

For operators who need fail-closed behaviour during outages, the
correct knob is a Redis HA setup (sentinel, cluster, AWS ElastiCache
multi-AZ). The code itself doesn't ship a fail-closed mode — that
would conflate a cache outage with an attack and turn graceful
degradation into a self-inflicted incident.

## Operational checks

After deploying or changing limits, verify with a quick probe:

```bash
# Should return 200 with X-RateLimit-* headers populated.
curl -sI \
  -H "x-skill-pool-tenant: acme" \
  -H "Authorization: Bearer $TOKEN" \
  http://localhost:8080/v1/skills | grep -i x-ratelimit
```

To prove the 429 path works end-to-end without DoS-ing real traffic,
set a tiny limit on a test tenant first:

```bash
skill-pool-server admin tenant-create --slug rl-test --name "RL Test"
skill-pool-server admin tenant-rate-limits --slug rl-test --rpm 5 --burst 5
skill-pool-server admin token-create --tenant rl-test --name probe
# … then hammer /v1/skills with curl in a loop and watch the 429 kick in.
```

## Future work

* **Sliding-window** counters via Redis Lua (`INCRBY` + cell rotation)
  or the `redis-cell` module — fixes the boundary-doubling pathology.
* **Per-route limits** — bundle GETs (large, cacheable) and POSTs to
  `/v1/skills` (heavy: parse + storage + DB) want different ceilings.
* **Per-user limits inside a tenant** — guard against a single
  compromised token within a tenant. Today the tenant is the unit of
  isolation.
* **Cost-weighted tokens** — a bundle download should consume more
  budget than a list call. Today every request costs 1.
* **Cache the tenant lookup** through `cache::cached_json` once
  sister-agent A's helper lands; the per-cold-tenant SELECT is the
  one remaining DB cost in the limiter's hot path.
