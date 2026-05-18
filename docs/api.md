# HTTP API

> Phase 1 surface. Endpoints marked **stub** return 501 Not Implemented; payloads documented for the client-side implementation to follow.

## Common

### Authentication

`Authorization: Bearer <token>` where `<token>` was issued for the resolved tenant.

### Tenant resolution

By subdomain (`acme.skill-pool.example.com`) or `X-Skill-Pool-Tenant: acme` header. See `docs/tenancy.md`.

### Errors

```json
{
  "error": "tenant_resolution_failed",
  "message": "missing Host header"
}
```

Codes: `not_found`, `unauthorized`, `forbidden`, `bad_request`, `tenant_resolution_failed`, `not_implemented`, `internal_error`.

---

## `GET /v1/healthz`

No auth. No tenant. Liveness + dependency status.

```json
{ "status": "ok", "db": "up", "version": "0.1.0" }
```

`db` may be `"down"` during transient blips — the endpoint stays HTTP 200 so the LB doesn't pull the node out of rotation on a 200ms blip. Page on `down` from your monitoring system, not from `/healthz`.

---

## `GET /v1/skills` — list (stub)

Query params: `query`, `tags` (comma-separated), `limit` (default 50, max 200).

Response: `[{ "slug": "...", "version": "...", "description": "..." }, ...]`

## `GET /v1/skills/{slug}` — get one (stub)

Response: same shape, plus version history (when implemented).

## `GET /v1/skills/{slug}/bundle.tar.gz` — download (stub)

Streams the bundle or 302-redirects to a short-lived signed URL on object storage.

## `POST /v1/skills` — publish (stub)

Multipart:
- `bundle` — the gzipped tar containing `SKILL.md` at the root
- `metadata` — JSON: `{ "slug", "version", "description", "tags": [...] }`

Server validates:
- SKILL.md present + frontmatter parses
- `description` length ≤ 1536
- No `/home/`-style absolute paths in body
- Secret scan (gitleaks rules)
- SHA-256 of bundle stored alongside

Response: created skill row.

## `POST /v1/skills/validate` — lint without persist (stub)

Same payload as publish; returns validation result without storing. Used by the web editor's "Validate" button.
