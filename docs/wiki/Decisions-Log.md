# Decisions Log

> ADR-style record of the major architectural and product decisions
> behind skill-pool. Each entry follows: context → decision →
> consequences → status. Synthesized from the merge history; the
> source-of-truth is the commit log and the per-PR design notes.

## DEC-001 — Multi-tenancy from row 1, not as an afterthought

**Date:** 2026-05-12 · **Status:** Accepted

### Context

A skill catalog hosted by a vendor (or by a central platform team)
needs to support multiple tenants by definition. Retrofitting tenant
isolation onto a single-tenant codebase has a notoriously bad track
record (every business query becomes a possible cross-tenant leak).

### Decision

Every business table carries `tenant_id` from migration `0001`.
Every SQL query filters by `tenant_id`. A static-analysis test
harness (`server/tests/tenant_scoping.rs`, PR #8 §L17) walks every
`sqlx::query*!` macro call and asserts the predicate at build time.

### Consequences

**Positive:** Cross-tenant leaks are a build error, not a postmortem
finding. Adding a new endpoint requires the developer to think
about tenant scope.

**Negative:** Every new table needs a `tenant_id` column. Some
purely-global tables (the global rate-limit registry) get a synthetic
"system" tenant row.

---

## DEC-002 — Stateless server; all shared state in Postgres + object storage

**Date:** 2026-05-12 · **Status:** Accepted

### Context

Horizontal scale and zero-downtime deploys are easier if the server
process holds no state. The previous-generation "registry" we
benchmarked against carried in-process caches that had to be warmed
on every replica boot.

### Decision

The server is a pure Axum app. All shared state lives in Postgres
(metadata, audit log) and opendal-backed object storage (bundles).
Redis is opt-in for rate limits, cache, queue — server falls back
gracefully when absent (caches no-op, rate limits fail-open, jobs
run inline).

### Consequences

**Positive:** `helm scale` from 1 to 5 replicas works without code
changes.

**Negative:** Some hot paths (theme lookup, custom-domain cache) do
hit a per-process LRU. Each one is documented and has a TTL <60s so
multi-replica deploys eventually converge.

---

## DEC-003 — No auto-migrate on server startup

**Date:** 2026-05-13 · **Status:** Accepted

### Context

Servers that run `sqlx migrate run` on every boot make it possible
for a broken deploy (e.g. a botched migration) to run a destructive
migration on production as a side effect. The blast radius is
unacceptable.

### Decision

Migrations are a separate operator step. The binary's `migrate`
subcommand exists; the boot path does not call it. The Helm chart's
`pre-upgrade` hook runs migrations explicitly so the normal flow
still feels seamless.

### Consequences

**Positive:** A failed app deploy never alters the schema.
Forward-only migrations are easier because the operator chooses when
to apply them.

**Negative:** First-time operators sometimes forget the migrate step
and see a 500 on `/v1/healthz`. Documented in
`docs/deploy/single-node.md` and `docs/deploy/aws.md`.

---

## DEC-004 — `Host` header for tenant resolution, not URL path

**Date:** 2026-05-13 · **Status:** Accepted

### Context

Three options for routing tenants: URL path prefix
(`/tenants/acme/...`), URL subdomain (`acme.skill-pool.example.com`),
or HTTP header (`X-Skill-Pool-Tenant: acme`).

### Decision

Subdomain (`Host` header) is the production path, with the header as
a development fallback. Custom domains layer on top via a cache
that's checked before the subdomain logic.

### Consequences

**Positive:** Cookies, CORS, and WebAuthn scope to the subdomain
naturally. Each tenant gets a clean URL.

**Negative:** Wildcard DNS + wildcard TLS cert is required (or
on-demand TLS via Caddy/Traefik). Local dev needs `localtest.me` (or
`/etc/hosts` entries).

---

## DEC-005 — `Authorization: Bearer` tokens, no cookies for the API

**Date:** 2026-05-14 · **Status:** Accepted

### Context

API consumers are CLI tools, CI jobs, and the SvelteKit portal. The
portal can hold a session cookie; the others can't. Two parallel
auth schemes (cookie + bearer) double the test surface.

### Decision

Bearer tokens everywhere. The portal exchanges the OIDC/SAML session
for a bearer token via a server-side fetch and includes it on every
API call.

### Consequences

**Positive:** One auth path. CSRF is moot (no cookies → no
session-riding).

**Negative:** Token lifecycle (rotation, revocation) is the operator's
problem. The "personal tokens" page (#4) gives users self-service
mint + revoke for this reason.

---

## DEC-006 — opendal for object storage, not a hand-rolled adapter

**Date:** 2026-05-14 · **Status:** Accepted

### Context

The catalog needs to write bundles to a backend. We could pick S3
specifically (most users would be on AWS) or write to all of S3 +
GCS + Azure Blob + local fs.

### Decision

Use `opendal` (Apache Foundation). One trait, many backends. The
`SKILL_POOL_STORAGE_URI` env var picks the backend at runtime —
`fs://`, `s3://`, `gcs://`, `azblob://`, etc.

### Consequences

**Positive:** Local dev uses `fs://`, prod can be on any cloud,
single-node deploys can hold bundles on a local volume.

**Negative:** opendal's pre-signed URL support is uneven across
backends. The bundle endpoint has two response shapes (307 redirect
vs streamed body) to paper over this.

---

## DEC-007 — Postgres + pgvector, not a dedicated vector DB

**Date:** 2026-05-15 · **Status:** Accepted

### Context

Semantic search and embedding-based dedup need a vector index. Two
options: add a dedicated vector DB (Qdrant, Weaviate, Milvus) or
extend Postgres.

### Decision

Postgres + pgvector. The `vector(384)` column is added to `skills`;
`description_embedding <=> $1` returns cosine similarity. A dedicated
vector DB would mean two storage systems, two backup pipelines, two
recovery scenarios.

### Consequences

**Positive:** One DB to back up, one to monitor. Existing SQL
tooling works.

**Negative:** Hosting tier needs to ship pgvector preloaded (RDS,
GCP CloudSQL, Azure all do). Hand-rolled Postgres deploys need
`CREATE EXTENSION vector`. fastembed (the embedder) is opt-in
behind a Cargo feature so default builds don't pull in ONNX.

---

## DEC-008 — Two-stage LLM capturer (Haiku → Sonnet), not single-pass

**Date:** 2026-05-16 · **Status:** Accepted

### Context

The Phase 4.6 capturer turns scored sessions into SKILL.md drafts.
Single-pass with Sonnet costs ~10x more than Haiku per session. Most
sessions (estimate: ~70%) are not generalizable — they're noise.

### Decision

Stage 1: Haiku extracts structured JSON ({ problem, solution_steps,
generalizable, scope, preconditions }). If `generalizable: false`,
stop. Stage 2: Sonnet, only on the ~30% pass-through, writes
SKILL.md.

### Consequences

**Positive:** Cost per session is dominated by the cheap pass. Wrong
answers from Stage 1 ("this is generalizable" when it isn't) are
caught by the human reviewer at draft publish — no autonomous
publish.

**Negative:** Two model calls per accepted session means two
prompt-engineering surfaces. The prompts live in
`cli/src/capturer.rs` and are versioned with the binary.

---

## DEC-009 — Curator-in-the-loop publish, not autonomous

**Date:** 2026-05-16 · **Status:** Accepted

### Context

An LLM that drafts skills could also auto-publish them. We could
either trust the LLM (high precision dependency) or always require a
human review.

### Decision

The capturer creates **drafts**, not skills. A curator clicks
**Publish** in the inbox, assigning a version. The draft only ever
becomes a `skills` row via that explicit action.

### Consequences

**Positive:** The system can be wrong (Stage 1 says "generalizable"
when it isn't) without polluting the catalog. Audit trail is
explicit.

**Negative:** Curator latency is a bottleneck. Some teams will set
up a Slack webhook + `draft.create` notification so the inbox gets
attention quickly.

---

## DEC-010 — `kind` discriminator on one catalog table, not three

**Date:** 2026-05-17 · **Status:** Accepted

### Context

Phase 5 added agents and slash-commands. Two implementations: three
parallel tables with identical schemas, or one table with a `kind`
column.

### Decision

One table, `kind IN ('skill', 'agent', 'command')`. The dependency
graph, decay model, embedding, and audit log all share the same
schema.

### Consequences

**Positive:** Migrations are simpler. Adding a fourth kind (hooks?
templates?) is a CHECK constraint change, not a new table.

**Negative:** Some endpoints need a `?kind=` query param. Decay
heuristics are tuned per-kind (today: skills only).

---

## DEC-011 — Optional git mirror, never authoritative

**Date:** 2026-05-17 · **Status:** Accepted

### Context

Some teams want a human-readable history of the catalog on disk
(audit, code review, regulatory). Should the catalog write to git
synchronously?

### Decision

`SKILL_POOL_GIT_REPO_PATH` enables a best-effort fire-and-forget git
commit on every publish. Postgres is the source of truth; git is a
mirror. Publish never blocks on the git side.

### Consequences

**Positive:** Teams that want the audit get it; teams that don't
pay zero cost. A failed git commit is a logged warning, never an
HTTP 5xx.

**Negative:** Git mirror can drift from Postgres if the spawned
process dies between INSERT and commit. There's no reconciler today
(future work).

---

## DEC-012 — Forward-only migrations, no down-migrations

**Date:** 2026-05-18 · **Status:** Accepted

### Context

Down-migrations are a maintenance burden (every up needs a tested
down) and rarely fire in anger — production rollbacks are almost
always "revert the app, schema stays".

### Decision

`sqlx` migrations are forward-only. Schema changes are additive
(new columns, new tables, new indexes). A "drop column" lives across
two releases: stop writing to it in release N, drop it in release
N+1.

### Consequences

**Positive:** Half the migration code. App rollback (`helm
rollback`) works because the old binary reads the new schema fine.

**Negative:** Disaster-recovery for "we shipped a bad migration" is
"point-in-time restore from snapshot". Documented in
`docs/ops/rollback.md`.

---

## DEC-013 — `nip.io` + Let's Encrypt as the default TLS path on AWS

**Date:** 2026-05-19 · **Status:** Accepted

### Context

The AWS Terraform module should give a working HTTPS deploy without
the operator buying a domain or setting up Route53 ALIAS records.

### Decision

Default deploy uses `<dashed-ip>.nip.io` for DNS — a free wildcard
service that resolves `*.<dashed-ip>.nip.io` to any IPv4. cert-manager
runs in-cluster with the HTTP-01 challenge and writes a Let's Encrypt
cert into a Secret the ALB serves. `var.use_acm_cert = true` flips
to ACM when the operator owns a real domain.

### Consequences

**Positive:** Zero-domain-purchase first deploy. Useful for
demos, evaluation, and small teams.

**Negative:** nip.io is third-party — if it goes down, new tenant
subdomains can't issue certs. Documented; operators should swap to
a real domain before production traffic.

---

## DEC-014 — Redis is opt-in, fail-open everywhere it's used

**Date:** 2026-05-19 · **Status:** Accepted

### Context

Redis adds a moving part. A homelab deploy doesn't want it. A
production deploy needs it for rate limits + cache + queue.

### Decision

`SKILL_POOL_REDIS_URL` is optional. When unset (or unreachable), the
server:

- Cache layer no-ops (every read hits Postgres).
- Rate limits fail-open (every request allowed).
- Queue jobs run inline on the writer's thread.

### Consequences

**Positive:** Single-node deploys don't need Redis. Mid-tier deploys
get caching + rate limiting for free when they add it. Production
deploys get queue durability when they need it.

**Negative:** "Fail open on rate limit" is a security trade-off —
during a Redis outage, a noisy tenant can exhaust DB resources.
Documented; mitigated by per-tenant Postgres-level limits as a
backstop.

---

## DEC-015 — Honour-system `logo_uri` plus uploaded `logo_storage_key`

**Date:** 2026-05-19 · **Status:** Accepted

### Context

Some tenants want to host their logo on their own CDN. Others want
to upload one and have the registry host it.

### Decision

Both. `logo_uri` is a TEXT column the server does not fetch or
validate. `logo_storage_key` points at a server-hosted SVG/PNG/JPEG/WEBP
the server sanitizes on upload. The client falls back to `logo_uri`
when `logo_storage_key` is NULL.

### Consequences

**Positive:** Lowest-friction onboarding (paste a URL); secure path
available (upload + sanitize).

**Negative:** Tenants who paste a malicious URL can serve script-like
content. CSP is the second line of defense (documented in
`docs/enterprise/asset-cdn.md`). The honour-system caveat is
explicit in [Theming](Theming.md).

---

## DEC-016 — Web stack: SvelteKit, not React or Solid

**Date:** 2026-05-20 · **Status:** Accepted

### Context

The portal needs SSR (for the theme-aware shell) and a small bundle.
React + Next.js has the largest pool of available components; Solid
has the best perf; SvelteKit has the simplest mental model.

### Decision

SvelteKit on adapter-node. Theme resolution in `hooks.server.ts`,
form actions for mutations (server-only `+page.server.ts`),
component scope via the `$lib/server` boundary.

### Consequences

**Positive:** The portal is ~500KB gzipped. Theme injection is a
single `<style>` block in `<svelte:head>`.

**Negative:** SvelteKit's CSRF + Host-header policies bit us
multiple times (see [FAQ](FAQ.md)). Each one had a fix in 2 lines of
config.

---

## Where to read next

- [Architecture](Architecture.md) — how DEC-001 through DEC-016 fit together
- [FAQ](FAQ.md) — the consequences of these decisions in practice
- [Operator Guide](Operator-Guide.md) — deploy paths that follow from them

## Cross-links into the codebase

- Source for the synthesized decisions: `git log --oneline --all`
- PR-specific design notes: each merge commit message ("feat(...): ...")
- `docs/architecture.md` — the canonical architecture summary
- `server/migrations/` — the schema evolution that DEC-001 + DEC-012
  drive
