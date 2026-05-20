# Multi-Tenancy

> skill-pool is multi-tenant from row 1. Every business-data table
> carries `tenant_id`, every object-storage key is tenant-prefixed,
> and a static-analysis test harness asserts the invariant at build
> time. This page is the canonical reference for how tenants are
> resolved, isolated, and (when needed) physically separated.

## Modes

### Shared (default)

One deploy, many tenants. Every row in every business-data table
carries `tenant_id`. Tenant identity is resolved per-request from the
`Host` header (subdomain) or `X-Skill-Pool-Tenant` header (dev
fallback). Object storage keys are namespaced under `{tenant_id}/`.
Per-tenant API tokens are stored in `tenant_api_tokens`.

This is what the free / Team tier runs on, and what most Enterprise
tenants will run on too. Isolation is enforced in code and in tests —
not by physical separation.

### Dedicated (Enterprise opt-in)

Same Docker image, different DSN. The operator sets:

```bash
SKILL_POOL_TENANCY_MODE__MODE=dedicated
SKILL_POOL_TENANCY_MODE__TENANT_SLUG=acme
```

The tenant extractor returns the pinned slug regardless of headers.
The operator runs a separate Postgres and object-storage backend for
this deploy; the same migration set applies. Useful for:

- Data-residency requirements (EU-only Postgres, EU-only S3).
- Compliance regimes that need physical separation.
- Tenants who insist on operating their own database.

Migrating a tenant from shared → dedicated is a documented playbook:
see `docs/enterprise/migration-shared-to-dedicated.md`.

## Tenant resolution algorithm

The extractor is `TenantCtx::from_request_parts` in
`server/src/tenant.rs`. Pseudo-code:

```text
if mode == dedicated:
    return pinned slug

if custom_domain_cache contains Host header:
    return cached tenant slug

if X-Skill-Pool-Tenant header present and non-empty:
    return lower(header)

if Host header has a non-empty leading label that is not "www":
    return lower(leading label)

else:
    reject with 400 tenant_resolution_failed
```

The slug is then resolved against `tenants(slug, status='active')`. A
missing or suspended tenant returns **401 unauthorized** — we
deliberately do not leak the existence of a slug to unauthenticated
callers.

### Why `Host` first, then `X-Skill-Pool-Tenant`?

Subdomain routing is what production uses. The header path exists for:

- Local development against `localtest.me` or `127.0.0.1`.
- Test fixtures (the test helpers all set the header).
- Operators debugging a tenant by curl without needing to set up DNS.

Custom domains (Enterprise only) take precedence over both — see
[Custom-Domain-ACME](Custom-Domain-ACME.md).

## Subdomain routing

Default origin pattern: `https://{tenant}.skill-pool.example.com`.

In local dev we use `localtest.me`, which resolves to `127.0.0.1`
publicly, so `acme.localtest.me` and `globex.localtest.me` both work
without `/etc/hosts` edits.

Phase 2 adds **custom domain** support: tenants can CNAME their own
domain. ACME issuance happens automatically via Caddy/Traefik's
on-demand TLS hook, gated on the registry's `cert-ok` endpoint. See
[Custom-Domain-ACME](Custom-Domain-ACME.md).

## API token model

- Created via the admin CLI (`skill-pool-server admin token-create`)
  or the web UI; the raw token is shown **once**.
- Stored in DB as SHA-256 hex of the raw bytes; the raw secret is never
  persisted.
- Scope is a space-separated capability list (e.g.
  `skills:read skills:publish` for a CI publisher, `tenant:admin` for an
  admin token).
- Tokens are **tenant-bound** — there's no cross-tenant token. A
  developer who belongs to two tenants holds two tokens, one per
  tenant. The CLI keeps tenant-namespaced sections in
  `~/.skill-pool/config.toml`.

A token presented for the wrong tenant returns 401 — see "Isolation
guarantees" below.

### Personal vs tenant-scoped tokens

Two token kinds share the same table:

| Kind | Created via | Use for | Listed in |
|---|---|---|---|
| Tenant-scoped | `admin token-create` | CI, service accounts, bootstrap | `tokens` table |
| Personal | `/v1/profile/tokens` (web UI) | Developer's CLI | `personal_api_tokens` table |

Both live in `Authorization: Bearer spk_…` headers. The server
distinguishes them by table lookup, not by token prefix.

## Isolation guarantees (Phase 1)

1. **Auth verifies tenant binding.** Tenant resolution happens
   **before** auth, but the auth middleware checks that the bearer
   token belongs to the resolved tenant. A token for `acme` presented
   against `globex.skill-pool` returns 401.

2. **Every SQL query filters by `tenant_id`.** No global state, no
   helper that "remembers" the last tenant. Each `query_as!` /
   `query!` invocation must include `WHERE tenant_id = $1`. The
   static-analysis harness at `server/tests/tenant_scoping.rs`
   walks every `sqlx::query*!` macro call in the crate and asserts
   the predicate. PRs that omit it fail CI.

3. **Object storage keys are tenant-prefixed.** Keys take the shape
   `{tenant_id}/{slug}/{version}.tar.gz`. There is no flat namespace
   where one tenant could enumerate another's bundles even with a
   stolen pre-signed URL — keys must be known to be retrieved, and
   pre-signed URL generation is itself tenant-scoped.

4. **Audit events carry `tenant_id`.** The audit log filters and
   exports (SIEM, retention policies) all key off it. Cross-tenant
   audit queries are not possible through any documented endpoint.

5. **Rate limiting is per-tenant.** Token bucket counters in Redis
   are keyed by `{tenant_id}:{window}`. A noisy tenant cannot exhaust
   the global budget. See `server/src/rate_limit.rs`.

## Background tasks and tenant scope

Long-lived background tasks (decay sweep, webhook delivery, email
DLQ, queue worker) all iterate tenants explicitly:

```rust
for tenant in active_tenants(&db).await? {
    sweep_decay(&tenant.id, &db).await?;
}
```

They do not operate on the global state; if a row's `tenant_id` got
corrupted it would be skipped. The static-analysis harness covers
these paths too.

## What Phase 1 did NOT provide (now shipped)

- SSO (SAML/OIDC) — **shipped in Phase 2**, see [SSO-Setup](SSO-Setup.md).
- SCIM provisioning — **shipped in Phase 2**, see `docs/scim.md`.
- Per-tenant rate limits and quotas — **shipped in Phase 5**.
- Cross-region replication / data residency tagging — **shipped in
  Phase 5** via `tenants.storage_uri` and `tenants.region`. See
  `docs/enterprise/data-residency.md`.

## Tenant lifecycle (operator verbs)

| Verb | Command |
|---|---|
| Create | `skill-pool-server admin tenant-create --slug acme --name "Acme Inc."` |
| List | `skill-pool-server admin tenant-list` |
| Suspend | `skill-pool-server admin tenant-suspend --tenant acme` (suspends sign-ins) |
| Delete | `skill-pool-server admin tenant-delete --tenant acme --confirm acme` (purges DB + S3 keys, audit-logged) |

Tenant create is idempotent on `slug` (returns the existing row).
Delete is irreversible — see `server/src/admin.rs` for the audit-event
shape it emits before the purge.

## Where to read next

- [Tenant Onboarding](Tenant-Onboarding.md) — first-time playbook
- [SSO Setup](SSO-Setup.md) — federation against Okta/Azure/Google/Authentik
- [Custom Domain + ACME](Custom-Domain-ACME.md) — `skills.acme.com` →
  registry
- [Architecture](Architecture.md) — where tenant resolution sits in the
  request pipeline

## Cross-links into the codebase

- `server/src/tenant.rs` — extractor + custom-domain cache
- `server/migrations/0001_tenants.sql` — base schema
- `server/tests/tenant_scoping.rs` — static-analysis harness (#8 §L17)
- `server/src/admin.rs` — admin CLI verbs
- `docs/tenancy.md` — the original tenancy note this page mirrors
- `docs/enterprise/dedicated-mode.md` — physical-isolation toggle
- `docs/enterprise/data-residency.md` — per-tenant region tag +
  storage URI override
