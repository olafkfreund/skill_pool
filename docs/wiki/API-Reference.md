# API Reference

> Every HTTP endpoint exposed by `skill-pool-server`, grouped by
> resource. Mirrored from `docs/api.md` in the repo with the
> additional endpoints shipped in #4 (versions, SSO admin, profile
> tokens) appended.

## Common

### Authentication

`Authorization: Bearer <token>` where `<token>` was issued for the
resolved tenant. See [Multi-Tenancy](Multi-Tenancy.md#api-token-model).

### Tenant resolution

By subdomain (`acme.skill-pool.example.com`), `X-Skill-Pool-Tenant:
acme` header, or custom-domain mapping. See
[Multi-Tenancy](Multi-Tenancy.md#tenant-resolution-algorithm).

### Errors

```json
{
  "error": "tenant_resolution_failed",
  "message": "missing Host header"
}
```

Codes: `not_found`, `unauthorized`, `forbidden`, `bad_request`,
`tenant_resolution_failed`, `not_implemented`, `internal_error`.

---

## Health

### `GET /v1/healthz`

No auth. No tenant. Liveness + dependency status.

```json
{
  "status": "ok",
  "version": "0.1.0",
  "deps": {
    "db":       { "status": "up",  "latency_ms": 3  },
    "storage":  { "status": "up",  "latency_ms": 12 },
    "embedder": { "status": "off" }
  }
}
```

`deps.<name>.status` is one of `"up"`, `"down"`, or `"off"`. Top-level
`status` is `"ok"` when every dep is `"up"` or `"off"`, `"degraded"`
when a required dep (`db`) is `"down"`. **HTTP 200 always** — page on
`deps.db.status == "down"` from your monitor; the load balancer
shouldn't pull the node on a transient blip.

---

## Skills (catalog)

### `GET /v1/skills` — list / search

| Param            | Type      | Description                                          |
|------------------|-----------|------------------------------------------------------|
| `query`          | string    | ILIKE substring match against slug + description     |
| `tags`           | csv       | All tags must be present                              |
| `limit`          | int       | Default 50, clamped 1..200                            |
| `semantic`       | string    | Rank by cosine similarity (Phase 5 + `--features fastembed`) |
| `min_similarity` | float     | Min cosine similarity (0.0..1.0) when `semantic` set  |
| `kind`           | string    | `skill` (default), `agent`, or `command`              |

When `semantic` is omitted, results are ordered by `slug, created_at
DESC`. When set, results carry a `similarity` field in `[0.0, 1.0]`.
`semantic` takes precedence over `query`; `tags` composes with either.

### `GET /v1/skills/{slug}` — get one

Returns the canonical metadata for the latest published version. Same
shape as a `GET /v1/skills` row plus a `kind` discriminator and the
embedded SKILL.md frontmatter.

### `GET /v1/skills/{slug}/bundle.tar.gz` — download

Two response shapes depending on storage backend:

| Backend                       | Response                                           |
|-------------------------------|----------------------------------------------------|
| `s3://`, `gcs://`, `azblob://`| **307 Temporary Redirect** to a 5-minute presigned URL |
| `fs://`                       | **200 OK** with `Content-Type: application/gzip` body |

Query params: `kind` (default `skill`), `bytes=true` to force the
streaming path regardless of backend.

Both paths emit a `download` event in `skill_usage_events` (used by
the decay model). Pre-signed URLs expire in 300s — clients should
not cache them.

### `GET /v1/skills/{slug}/skill-md` — fetch body

`text/plain` body containing SKILL.md (frontmatter + body). Emits a
`view` event. Tenant-scoped, auth required.

### `GET /v1/skills/{slug}/versions` — version history (#4)

Returns every published version of a skill (across all lifecycle
statuses), newest-first, capped at 50 rows. Powers the skill-detail
page's "Version history" table.

```json
[
  {
    "version": "2.0.0",
    "published_at": "2025-05-19T12:34:56Z",
    "change_summary": "rewrite for axum 0.8",
    "status": "published"
  }
]
```

- Tenant-scoped via the standard extractor.
- `?kind=skill|agent|command` — default `skill`.
- `published_by` carries `users.email` when known; omitted when NULL.
- `change_summary` is the row's `description` truncated to 200 chars.
- 404 when no row exists for the slug.

### `GET /v1/skills/{slug}/deps` — dependency closure

Transitive dependency closure of a published skill. Cycle-safe (UNION
dedup, depth cap of 10).

```json
[
  { "slug": "axum-extractor",  "version_range": "*",     "depth": 1 },
  { "slug": "sqlx-migrations", "version_range": "1.0.0", "depth": 2 }
]
```

Forward references are kept — `requires_slug` that isn't yet
published still appears in the closure so the CLI can warn-and-skip.

Declare dependencies in SKILL.md frontmatter:

```yaml
---
name: my-skill
requires:
  - axum-extractor             # latest
  - sqlx-migrations@1.0.0      # exact
---
```

### `POST /v1/skills` — publish

Multipart:
- `bundle` — the gzipped tar containing `SKILL.md` at the root.
- `metadata` — JSON: `{ "slug", "version", "description", "tags",
  "kind": "skill"|"agent"|"command" }`.

Server validates: SKILL.md present + frontmatter parses, `description`
≤ 1536 chars, no `/home/`-style absolute paths in body, gitleaks
secret scan, SHA-256 of bundle stored alongside, and version-range
conflict against existing dependencies (409 on conflict).

### `POST /v1/skills/validate` — lint without persist

Same payload as publish; returns the validation result without
storing. Used by the web editor's "Validate" button.

### `POST /v1/skills/{slug}/archive` — archive (admin)

`tenant:admin` scope required. Flips the latest published version's
`status` to `archived`. Catalog list automatically filters archived
skills out. Returns `{ slug, version }`. 404 when no published
version exists.

### `POST /v1/usage` — CLI-driven view event

Body: `{ skill_id, kind, event, project_hash }`. Called by
`skill-pool ensure` once per successful install; lets the decay model
see session-load activity. Best-effort: errors logged at `debug`,
never block.

---

## Drafts (Phase 4)

All draft endpoints are tenant-scoped via the standard extractor. GET
requires `skills:read`; POST requires `skills:publish`.

### `POST /v1/drafts` — create

Multipart: `metadata` JSON + `bundle` .tar.gz. Server validates the
same way as publish, then INSERTs into `drafts` with
`status='pending'`. Computes a `description` embedding (if
`--features fastembed`) and runs the dedup pass against published
skills — if similarity ≥ 0.85, the response carries
`merge_proposal_slug` + `merge_proposal_similarity`.

Fires the tenant's `draft.create` webhook (fire-and-forget, audit-logged).

### `GET /v1/drafts?status=pending`

Filters: `pending`, `published`, `discarded`, `all`. Returns the
draft inbox view.

### `GET /v1/drafts/{id}` — fetch one

### `GET /v1/drafts/{id}/skill-md` — render SKILL.md

`text/plain` SKILL.md extracted from the bundle.

### `POST /v1/drafts/{id}/publish`

Body: `{ "version": "1.0.0", "slug": "override" }` (slug optional).
Atomically: copies the bundle to the canonical key, INSERTs into
`skills` (rolls back on collision), UPDATEs the draft to `published`.
Re-publishing the same draft 400s; reusing `(slug, version)` 400s
with a "pick a different version" message.

### `POST /v1/drafts/{id}/discard`

Soft-delete: marks the draft `discarded` (kept for telemetry). Bundle
is purged from object storage.

---

## Tenants (admin)

All endpoints require `tenant:admin` scope.

### `GET /v1/tenants` — list (super-admin)

Cross-tenant list for the operator. Hidden behind a SUPER_ADMIN
token-scope gate.

### `GET /v1/tenant/skills/decay` — decay candidates

| Param      | Default | Description                                            |
|------------|---------|--------------------------------------------------------|
| `days`     | 180     | Stale-for-N-days threshold (max 1825).                 |
| `max_uses` | 3       | Return rows with `use_count < max_uses`.               |
| `limit`    | 200     | Max rows (max 1000).                                   |

Response sorted by `last_used_at ASC`. See [Phase-5-Lifecycle](Phase-5-Lifecycle.md#decay-rules).

### `GET /v1/tenant/usage/timeline`

| Param  | Default | Description                                |
|--------|---------|--------------------------------------------|
| `days` | 30      | Window (clamped 1..365).                   |

Per-day buckets, missing days zero-filled. Response: `[{ day,
downloads, views, unique_skills }]`.

### `GET /v1/tenant/usage/top`

| Param   | Default | Description                            |
|---------|---------|----------------------------------------|
| `days`  | 30      | Window (clamped 1..365).               |
| `limit` | 10      | Max rows (1..100).                     |

Response sorted by total events desc.

### `GET /v1/tenant/notifications/pending-count`

Returns `{ count: int }` — the pending-draft count rendered as the
sidebar badge in the web portal.

### `PUT /v1/tenant/notifications`

Body: `{ "webhook_url": "https://hooks.slack.com/…", "webhook_secret":
"optional" }`. Configures the draft-create webhook. With a secret,
deliveries are signed with `HMAC-SHA256` and the digest shipped in
`X-Skill-Pool-Signature: sha256=<hex>`.

### Custom domains

| Method | Path                                            | Description                                  |
|--------|-------------------------------------------------|----------------------------------------------|
| POST   | `/v1/tenant/custom-domains`                     | Claim a hostname; returns TXT verification record |
| GET    | `/v1/tenant/custom-domains`                     | List this tenant's domains                    |
| POST   | `/v1/tenant/custom-domains/{id}/verify`         | Run DNS TXT lookup; flip pending → verified    |
| DELETE | `/v1/tenant/custom-domains/{id}`                | Withdraw a claim                              |
| GET    | `/v1/tenant/custom-domains/{host}/cert-ok`      | **No auth.** 200 if verified/active; reverse-proxy hook |

See [Custom-Domain-ACME](Custom-Domain-ACME.md).

---

## Tenant Projects

Per-codebase curated bundles of skills/agents/commands. See [Projects](Projects.md) for the full feature.

| Method | Path | Purpose |
|---|---|---|
| GET    | `/v1/tenant/projects`                          | List projects in the tenant. Includes `item_count` per row. **Scope:** `tenant:admin`. |
| POST   | `/v1/tenant/projects`                          | Create a project. Body: `{slug, name, description?, git_remote?}`. **Scope:** `tenant:admin`. |
| GET    | `/v1/tenant/projects/{slug}`                   | Detail with `items: [{slug, kind, position}]`. **Scope:** `tenant:admin`. |
| PATCH  | `/v1/tenant/projects/{slug}`                   | Partial update; body fields are all `Option<T>` — only present fields are written. **Scope:** `tenant:admin`. |
| DELETE | `/v1/tenant/projects/{slug}`                   | Delete project + cascade items. **Scope:** `tenant:admin`. |
| PUT    | `/v1/tenant/projects/{slug}/items`             | Replace item list. Body: `[{slug, kind}, …]`. Order is preserved as `position`. **Scope:** `tenant:admin`. |
| GET    | `/v1/projects/resolve?remote=<url>`            | CLI auto-discovery: resolve a project by normalized git remote. Returns `{slug, name}` or 404. **Scope:** any authenticated tenant member. |

### Bootstrap with project precedence

`GET /v1/bootstrap?project=<slug>&stack=<tags>` — Project items load as tier 0 (highest precedence), then existing curated → tagged → semantic tiers backfill up to the 8-result cap. Response gains `project: {slug, name}`. `?debug=1` adds `tier_breakdown.project` listing the project-attributed slugs.

A non-existent project slug is a soft fallback (no 404 — the response just falls through to the stack tiers).

---

## Tenant SSO (admin, #4)

All endpoints require `tenant:admin` scope. These power the admin
SSO config UI shipped in #4. See [SSO Setup](SSO-Setup.md) for the full
walkthrough.

### `GET /v1/tenant/sso/oidc`

Returns the current OIDC config (or `null`). The
`client_secret` field is **redacted** in responses.

```json
{
  "issuer": "https://acme.okta.com/oauth2/default",
  "client_id": "0oa...",
  "client_secret": null,
  "default_role": "publisher",
  "redirect_uri": "https://acme.skill-pool.example.com/v1/auth/oidc/acme/callback"
}
```

### `PUT /v1/tenant/sso/oidc`

Body: `{ issuer, client_id, client_secret, default_role }`. Validates
the issuer URL is reachable (`/.well-known/openid-configuration` returns
JSON). 400 on validation failure.

### `DELETE /v1/tenant/sso/oidc`

Clears the OIDC config. Existing OIDC sessions remain valid until they
expire (14 days); new sign-ins via the OIDC button 404.

### `GET /v1/tenant/sso/saml`

Returns the current SAML config (or `null`). The IdP signing
certificate is base64-encoded in the response.

### `PUT /v1/tenant/sso/saml`

Body: `{ idp_entity_id, idp_sso_url, idp_cert_pem, default_role }`.
Validates the PEM parses (multipart `idp_cert_pem` or hex-encoded
form-field). 400 on validation failure.

### `DELETE /v1/tenant/sso/saml`

Clears the SAML config.

---

## Theme

### `GET /v1/theme` — public

Returns the current `Theme` row. Used by the SvelteKit portal's
request-time theme resolver in `web/src/hooks.server.ts`. No auth
because the login page needs branding before anyone has signed in.

### `PUT /v1/theme` — admin

Body: full `Theme` JSON. Server-side WCAG AA contrast check on
`fg`/`bg`; UI checks the other three pairs. See [Theming](Theming.md).

### `GET /v1/theme/logo` / `GET /v1/theme/favicon` — public

Streams the uploaded asset. `Cache-Control: public, max-age=300`.

### `POST /v1/theme/logo` / `POST /v1/theme/favicon` — admin

Multipart with a single `file` part. Accepted types:
`image/svg+xml`, `image/png`, `image/jpeg`, `image/webp` (favicon
also accepts `image/x-icon`). Size cap: 256 KiB for logo, 64 KiB for
favicon. SVG runs through the hardened sanitizer in
`server/src/logo_sanitize.rs`.

### `DELETE /v1/theme/logo` / `DELETE /v1/theme/favicon` — admin

### `GET /v1/theme/fonts` — public

Returns `{ "allowed": [...] }` — the 12-entry Google-Fonts allowlist
that powers the font picker.

---

## Profile (developer, #4)

`tenant:user` scope (any signed-in user). These power the profile page
shipped in #4 — a developer can mint and revoke their own personal
API tokens for the CLI.

### `GET /v1/profile/tokens`

Returns this user's personal tokens (id, name, scope, created_at,
last_used_at). Raw secrets are **not** returned — only the metadata.

### `POST /v1/profile/tokens`

Body: `{ "name": "my-laptop", "scope": "skills:read skills:publish" }`.
Returns the freshly-minted raw token **once** (`spk_…`); the DB stores
SHA-256 only.

### `DELETE /v1/profile/tokens/{id}`

Revokes the token. Subsequent requests with it return 401.

---

## Auth (SSO)

### OIDC

| Method | Path                                        | Description                                     |
|--------|---------------------------------------------|-------------------------------------------------|
| GET    | `/v1/auth/oidc/{tenant}/start`              | Redirect to IdP with PKCE state                 |
| GET    | `/v1/auth/oidc/{tenant}/callback`           | IdP redirect target; exchanges code, mints session |

### SAML

| Method | Path                                       | Description                                  |
|--------|--------------------------------------------|----------------------------------------------|
| GET    | `/v1/auth/saml/{tenant}/metadata`          | SP metadata XML for IdP import               |
| POST   | `/v1/auth/saml/{tenant}/acs`               | ACS endpoint — validates signed assertion    |

Full setup walkthrough in [SSO Setup](SSO-Setup.md).

### Logout

| Method | Path             | Description                       |
|--------|------------------|-----------------------------------|
| POST   | `/v1/auth/logout`| Invalidate the active session     |

---

## SCIM 2.0 (Phase 2)

Tenant-scoped via `X-Skill-Pool-Tenant`. Auth via dedicated SCIM
bearer token (separate from API tokens; minted via
`skill-pool-server admin scim-token-create`).

| Method | Path                          | Description                              |
|--------|-------------------------------|------------------------------------------|
| GET    | `/v1/scim/v2/Users`           | List users (filter + pagination)         |
| GET    | `/v1/scim/v2/Users/{id}`      | Get one                                  |
| POST   | `/v1/scim/v2/Users`           | Create                                   |
| PATCH  | `/v1/scim/v2/Users/{id}`      | Partial update                           |
| DELETE | `/v1/scim/v2/Users/{id}`      | Deactivate                               |
| GET    | `/v1/scim/v2/Groups`          | List groups                              |
| ...    | (full SCIM 2.0 verbs)         | See `docs/scim.md`                       |

---

## MCP (Phase 5)

### `POST /v1/mcp`

JSON-RPC 2.0 adapter so a developer's Claude session can search the
team catalog without leaving the conversation. Same `Authorization:
Bearer …` + `X-Skill-Pool-Tenant: …` headers as the REST surface.

| Method | Purpose |
|---|---|
| `initialize` | Returns `{ protocolVersion, capabilities: { tools: {} }, serverInfo }` |
| `tools/list` | Returns the catalog tools below |
| `tools/call` | Dispatches `{ name, arguments }` |
| `ping` | Health |
| `notifications/*` | Acked silently |

Tools:

| Tool | Args | Returns |
|---|---|---|
| `search_skills` | `{ query?, tags?, semantic?, limit? }` | Human summary + fenced JSON dump |
| `get_skill`     | `{ slug }`                              | Rendered SKILL.md as text content |
| `install_skill` | `{ slug, kind? }`                       | Same bundle as REST + content blocks |

A missing slug returns `isError: true` with the message in the
result — not a JSON-RPC error — so the model can recover gracefully.

JSON-RPC errors: `-32601` method not found, `-32602` invalid params,
`-32603` internal error. `401` at the HTTP layer when the bearer
token is missing/invalid.

Full walkthrough in [MCP Integration](MCP-Integration.md).

---

## Where to read next

- [CLI Reference](CLI-Reference.md) — what each subcommand POSTs
- [Multi-Tenancy](Multi-Tenancy.md) — tenant resolution + token model
- [SSO Setup](SSO-Setup.md) — `/v1/tenant/sso/*` walkthrough
- [Phase-5-Lifecycle](Phase-5-Lifecycle.md) — `/v1/tenant/usage/*` + decay

## Cross-links into the codebase

- `server/src/routes/mod.rs` — the full route table
- `server/src/routes/skills.rs` — catalog endpoints
- `server/src/routes/drafts.rs` — Phase 4 inbox
- `server/src/routes/auth/` — OIDC + SAML handlers
- `server/src/routes/theme.rs` — theme + logo + favicon endpoints
- `server/src/routes/custom_domains.rs` — custom domain CRUD + cert-ok
- `server/src/routes/mcp.rs` — JSON-RPC adapter
- `docs/api.md` — original API note this page mirrors
