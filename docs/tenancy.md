# Tenancy

> Phase 1 scope. Phase 2 layers SSO/SCIM on top.

## Modes

### Shared (default)

One deploy, many tenants. Every row in every business-data table carries `tenant_id`. Tenant identity is resolved per-request from the `Host` header (subdomain) or `X-Skill-Pool-Tenant` header (dev fallback). Object storage keys are namespaced under `{tenant_id}/...`. Per-tenant API tokens are stored in `tenant_api_tokens`.

### Dedicated (Enterprise opt-in)

Same Docker image. Operator sets:

```
SKILL_POOL_TENANCY_MODE__MODE=dedicated
SKILL_POOL_TENANCY_MODE__TENANT_SLUG=acme
```

The tenant extractor returns the pinned slug regardless of headers. Operator runs a separate Postgres and object storage; same migrations apply. Useful for data-residency, compliance, and physical-isolation requirements without forking the codebase.

## Tenant resolution algorithm

```
if mode == dedicated:
    return pinned slug
else if X-Skill-Pool-Tenant header present and non-empty:
    return lower(header)
else if Host header has a non-empty leading label that is not "www":
    return lower(leading label)
else:
    reject with 400 tenant_resolution_failed
```

The slug is then resolved against `tenants(slug, status='active')`. A missing or suspended tenant returns 401 `unauthorized` — we deliberately do not leak the existence of a slug to unauthenticated callers.

## Subdomain routing

Default origin pattern: `https://{tenant}.skill-pool.example.com`. In local dev we use `localtest.me` which resolves to 127.0.0.1 publicly, so `acme.localtest.me` and `globex.localtest.me` both work without /etc/hosts edits.

Phase 2 adds **custom domain** support: tenants can CNAME their own domain. ACME issuance happens automatically via Caddy/Traefik.

## API token model

- Created via admin endpoint or web UI; raw token shown **once**.
- Stored in DB as SHA-256 hex of the raw bytes; raw never persisted.
- Scope is a space-separated capability list (e.g. `skills:read skills:publish`).
- Tokens are tenant-bound — there's no cross-tenant token. A developer who belongs to two tenants holds two tokens, one per tenant. The CLI keeps tenant-namespaced sections in `~/.skill-pool/config.toml`.

## Isolation guarantees (Phase 1)

1. Tenant resolution happens **before** auth, but auth verifies the token belongs to the resolved tenant. A token for `acme` presented against `globex.skill-pool` returns 401.
2. Every SQL query that reads or writes a business table filters by `tenant_id` from the extractor — no global state, no helper that "remembers" the last tenant.
3. Object storage keys are prefixed with `tenant_id`. There is no flat namespace where one tenant could enumerate another's bundles even with a stolen pre-signed URL — keys must be known to be retrieved, and signing is tenant-scoped.
4. Audit events carry `tenant_id`; the SIEM export filter (Phase 2/Enterprise) is also tenant-scoped.

## What Phase 1 does NOT yet provide

- SSO (SAML/OIDC) — Phase 2 / issue #4
- SCIM provisioning — Phase 2 / issue #8
- Cross-region replication / data residency tagging — Phase 5 / issue #8
- Per-tenant rate limits and quotas — Phase 5 / issue #10
