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

`deps.<name>.status` is one of `"up"`, `"down"`, or `"off"`.

- `"up"` — probe succeeded; `latency_ms` (integer) is the round-trip time.
- `"down"` — probe failed; `error` (string) contains the reason.
- `"off"` — dependency not configured; `note` (string, optional) may explain why.

Top-level `status` is `"ok"` when every dep is `"up"` or `"off"`, and `"degraded"` when any required dep (`db`) is `"down"`. The HTTP status code is always **200** so the load balancer does not pull the node on a transient blip — page on `deps.db.status == "down"` from your monitoring system instead.

**Migration note:** the old top-level `db: "up"` field has been removed. Clients should read `deps.db.status`.

---

## `GET /v1/skills` — list

Query params:

| Param            | Type      | Description                                                                          |
|------------------|-----------|--------------------------------------------------------------------------------------|
| `query`          | string    | ILIKE substring match against slug + description                                     |
| `tags`           | csv       | All tags must be present                                                             |
| `limit`          | int       | Default 50, clamped to 1..200                                                        |
| `semantic`       | string    | Rank by cosine similarity of `description_embedding` (Phase 5)                       |
| `min_similarity` | float     | Minimum cosine similarity (0.0..1.0) when `semantic` is set. Default 0.0             |
| `kind`           | string    | `skill` (default), `agent`, or `command`. The same catalog table holds all three.    |

When `semantic` is omitted, results are ordered by `slug, created_at DESC` and the response shape is unchanged from Phase 1.

When `semantic` is set, results are ordered by similarity descending and each entry carries a `similarity` field in `[0.0, 1.0]`. The server returns **400** if no embedder is configured (`semantic search is not enabled on this server`). Build with `--features fastembed` to enable.

Response (semantic):

```json
[
  {
    "slug": "axum-handler",
    "version": "1.0.0",
    "description": "Pattern for axum tenant-scoped extractors",
    "tags": ["rust"],
    "status": "published",
    "created_at": "2026-05-18T12:00:00Z",
    "similarity": 0.94
  }
]
```

`semantic` and `tags` compose — both filters apply. `semantic` and `query` (keyword) are mutually exclusive in effect: `semantic` takes precedence.

### Agents and commands (Phase 5+)

The `kind` discriminator lets one catalog row be a skill, an agent (Claude Code subagent), or a slash-command. All three share the same schema, validation, dependency graph, embedding column, and decay model. The other catalog endpoints (`GET /v1/skills/{slug}`, `/bundle.tar.gz`, `/skill-md`, `/detail`) accept the same `?kind=` query param; default is `skill`.

`POST /v1/skills` (publish) accepts an optional `"kind": "agent"|"command"` in the metadata JSON; omitting it defaults to `skill` so existing clients are unchanged.

Decay candidates and the MCP search adapter are skills-only for v1.

## `GET /v1/tenant/skills/decay` — decay candidates (admin)

`tenant:admin` scope required.

| Param      | Type | Description                                       |
|------------|------|---------------------------------------------------|
| `days`     | int  | Stale-for-N-days threshold. Default 180, max 1825 |
| `max_uses` | int  | Return rows with `use_count < max_uses`. Default 3 |
| `limit`    | int  | Max rows. Default 200, max 1000                    |

Response: `[ { slug, version, description, use_count, last_used_at, created_at } ]`. Sorted by `last_used_at ASC` so the stalest rows surface first.

## `POST /v1/skills/{slug}/archive` — archive a skill (admin)

`tenant:admin` scope required. Flips the latest published version's `status` to `archived`. Catalog list automatically filters archived skills out. Returns `{ slug, version }`. 404 when no published version exists.

Skill usage tracking: `GET /v1/skills/{slug}/bundle.tar.gz` (both the redirect path and the streamed-bytes path) bumps `use_count` and refreshes `last_used_at` server-side. Failure here is logged but never fails the response.

## `GET /v1/tenant/usage/timeline` — daily activity (Phase 5)

`tenant:admin` scope required.

| Param  | Type | Description                                  |
|--------|------|----------------------------------------------|
| `days` | int  | Window. Default 30, clamped to 1..365.       |

Response: per-day buckets, missing days zero-filled.

```json
[
  { "day": "2026-05-12T00:00:00Z", "downloads": 0, "views": 0, "unique_skills": 0 },
  { "day": "2026-05-13T00:00:00Z", "downloads": 5, "views": 2, "unique_skills": 2 }
]
```

## `GET /v1/tenant/usage/top` — top skills in window (Phase 5)

`tenant:admin` scope required.

| Param   | Type | Description                            |
|---------|------|----------------------------------------|
| `days`  | int  | Window. Default 30, clamped to 1..365. |
| `limit` | int  | Default 10, clamped to 1..100.         |

Response: skills sorted by total events desc.

```json
[
  { "slug": "axum-handler", "downloads": 5, "views": 2, "total": 7 },
  { "slug": "kafka-consumer", "downloads": 3, "views": 0, "total": 3 }
]
```

Events:
- `download` — `GET /v1/skills/{slug}/bundle.tar.gz` (both bytes + redirect paths bump)
- `view` — `GET /v1/skills/{slug}/skill-md`

Writes are best-effort: a DB blip on the events insert is logged but never blocks the response. Both `get_bundle` and `get_skill_md` now require an authenticated caller (Bearer token); existing clients already send one, but the contract is now strict.

## `GET /v1/skills/{slug}/versions` — version history (#4)

Returns every published version of a skill (across all lifecycle
statuses — `published`, `archive_candidate`, `archived`), newest-first,
capped at 50 rows. Powers the skill-detail page's "Version history"
table.

```json
[
  {
    "version": "2.0.0",
    "published_at": "2025-05-19T12:34:56Z",
    "change_summary": "rewrite for axum 0.8",
    "status": "published"
  },
  {
    "version": "1.1.0",
    "published_at": "2025-04-02T08:11:22Z",
    "change_summary": "second cut",
    "status": "published"
  }
]
```

- Tenant-scoped via the standard extractor.
- `?kind=skill|agent|command` — default `skill`.
- `published_by` carries `users.email` when known and is omitted when
  the row's `created_by` is NULL (the current publish path stores NULL;
  this field will populate once #4's follow-up wires the caller's
  `user_id` through).
- `change_summary` is the row's `description` truncated to 200 chars
  with an ellipsis. The schema doesn't carry a separate change-summary
  column.
- 404 when no row exists for the slug.

## `GET /v1/skills/{slug}/deps` — dependency closure (Phase 5)

Returns the transitive dependency closure of a published skill.

```json
[
  { "slug": "axum-extractor",  "version_range": "*",     "depth": 1 },
  { "slug": "sqlx-migrations", "version_range": "1.0.0", "depth": 2 }
]
```

- Tenant-scoped via the standard extractor.
- Cycle-safe (UNION dedups; depth cap of 10 is belt-and-braces).
- Forward references are kept: a `requires_slug` that doesn't yet have a
  published row still appears in the closure so the CLI can warn-and-skip.
- 404 when the parent slug has no published version.

### Declaring dependencies

Add a `requires:` block to your SKILL.md frontmatter at publish time:

```yaml
---
name: my-skill
description: …
requires:
  - axum-extractor             # latest version
  - sqlx-migrations@1.0.0      # exact version
---
```

Entry syntax: `slug` (defaults to `*`) or `slug@<version-range>`. Server v1
understands `*` and exact versions; anything else is stored verbatim and
the client picks "latest" if it doesn't recognize the syntax. Self-require
is a 400.

`skill-pool ensure` calls `/deps` for every manifest entry and installs
the closure plus the manifest entries themselves. Duplicates collapse on
slug.

## `POST /v1/mcp` — MCP transport (Phase 5)

JSON-RPC 2.0 adapter so a developer's Claude session can search the team catalog without leaving the conversation. Single POST endpoint; same `Authorization: Bearer …` + `X-Skill-Pool-Tenant: …` headers as the REST surface.

### Claude config

```json
{
  "mcpServers": {
    "skill-pool": {
      "type": "http",
      "url": "https://acme.skill-pool.example.com/v1/mcp",
      "headers": {
        "Authorization": "Bearer spk_…",
        "X-Skill-Pool-Tenant": "acme"
      }
    }
  }
}
```

### Methods

| Method | Purpose |
|---|---|
| `initialize` | Returns `{ protocolVersion, capabilities: { tools: {} }, serverInfo }`. |
| `tools/list` | Returns the two tools below. |
| `tools/call` | Dispatches `{ name, arguments }` to a tool. |
| `ping` | Acknowledges health. |
| `notifications/*` | Acked silently. |

### Tools

**`search_skills`** — args: `{ query?, tags?, semantic?, limit? }`. Returns content blocks: a human-readable summary followed by a fenced JSON dump for tool-savvy consumers. Mirrors `GET /v1/skills` semantics (semantic takes precedence over keyword; tags compose with either).

**`get_skill`** — args: `{ slug }`. Returns the rendered SKILL.md (frontmatter + body) as a single text content block. A missing slug returns `isError: true` with the message in the result — not a JSON-RPC error — so the model can recover gracefully.

### Errors

| Code | Meaning |
|---|---|
| `-32601` | Method not found |
| `-32602` | Invalid params |
| `-32603` | Internal error |

`401 Unauthorized` is returned at the HTTP layer when the bearer token is missing or invalid.

## `GET /v1/skills/{slug}` — get one (stub)

Response: same shape, plus version history (when implemented).

## `GET /v1/skills/{slug}/bundle.tar.gz` — download

Returns the published bundle for `{slug}`. Two response shapes depending on
storage backend:

| Backend                       | Response                                                    |
|-------------------------------|-------------------------------------------------------------|
| `s3://`, `gcs://`, `azblob://`| **307 Temporary Redirect** to a 5-minute presigned URL      |
| `fs://`                       | **200 OK** with `Content-Type: application/gzip` body       |

Query params:

| Param   | Type | Description                                                    |
|---------|------|----------------------------------------------------------------|
| `kind`  | str  | `skill` (default), `agent`, or `command`                       |
| `bytes` | bool | If `true`, force the streaming-bytes path regardless of backend.|

Use `?bytes=true` from corporate proxies that strip cross-origin redirects
or test harnesses asserting on `Content-Disposition`. The presigned URL
expires in 300 seconds — clients should not cache it; re-call the endpoint
to refresh.

Both paths emit a `download` event into `skill_usage_events` (used by the
decay model and `GET /v1/tenant/usage/*` aggregations) before returning,
so usage counts are correct regardless of the response shape.

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

---

## Plugins (Layer 3)

A plugin bundles one or more published skills/agents/commands plus inline
hook/MCP/LSP blobs into a single installable unit Claude Code consumes via
`/plugin marketplace add` + `/plugin install`. Conceptual overview:
[`docs/plugins.md`](plugins.md). Manifest reference:
[`docs/plugin-manifest-schema.md`](plugin-manifest-schema.md). Source of
truth for the routes below: `server/src/routes/plugins.rs`,
`server/src/routes/marketplace.rs`, `server/src/routes/plugin_git.rs`.

Authorization (mirrors `/v1/skills`):

| Route | Scope |
|---|---|
| `POST /v1/plugins` | `skills:publish` (granted to `curator`, `admin`) |
| `DELETE /v1/plugins/{slug}/versions/{version}` | `skills:publish` |
| `GET /v1/plugins*` | any authenticated tenant member |
| `GET /.claude-plugin/marketplace.json` | public (no auth) — rate-limited |
| `GET|POST /git/plugins/{slug}.git/...` | public (no auth) — rate-limited |

The public routes are reached by Claude Code's installer, which is
unauthenticated by design. See `docs/plugins.md#authorization`.

## `POST /v1/plugins` — publish

Publish a new plugin version. The body carries the canonical
`.claude-plugin/plugin.json` manifest plus the registry-side metadata
skill-pool needs to slot the row into the per-tenant marketplace.

Body:

```json
{
  "slug": "rust-axum-toolkit",
  "manifest": {
    "name": "rust-axum-toolkit",
    "version": "1.2.0",
    "description": "Curated skills, agents, and hooks for Rust + Axum",
    "tags": ["rust", "axum"]
  },
  "contents": [
    { "kind": "skill",   "slug": "rust-axum-handler",        "version": "1.2.3" },
    { "kind": "agent",   "slug": "sqlx-migration-reviewer",  "version": "0.4.0" },
    { "kind": "command", "slug": "deploy",                   "version": "0.1.0" }
  ],
  "sourcing_mode": "internal",
  "status": "published"
}
```

Field rules:

| Field | Required | Notes |
|---|---|---|
| `slug` | yes | Registry identifier. Distinct from `manifest.name` (which is the human-facing display name). |
| `manifest` | yes | JSONB body of `.claude-plugin/plugin.json`. `manifest.name`, `manifest.version`, `manifest.description` are all required and non-empty at publish time. Stored verbatim. Capped at 256 KiB serialized. |
| `contents[]` | no | Each entry's `(slug, kind, version)` must resolve to a `status='published'` row in the **same tenant**. Cross-tenant references rejected. `kind` ∈ `skill|agent|command`. |
| `sourcing_mode` | yes | `internal` (skill-pool hosts the bytes) / `external` (curator's git URL) / `mirror` (skill-pool clones + serves). |
| `external_git_url` | required when `sourcing_mode = external` | HTTPS git URL. |
| `upstream_url` | required when `sourcing_mode = mirror` | Upstream HTTPS git URL skill-pool pulls from. |
| `status` | no | `draft` or `published`. Defaults to `published`. `archived` is not a valid initial state — use the archive endpoint. |

Status mapping for publish-time validation (in check order):

| Failure | HTTP | Body |
|---|---|---|
| Manifest > 256 KiB serialized | 413 | `{"error": "payload_too_large", "message": "manifest is N bytes; limit is 262144"}` |
| Missing/empty `manifest.name|version|description` | 422 | Field-keyed error map: `{"name": "required and non-empty", ...}` |
| Invalid `sourcing_mode` enum | 400 | Lists allowed values. |
| `external` without `external_git_url`, or `mirror` without `upstream_url` | 422 | `{"external_git_url": "required when sourcing_mode=external"}` |
| `contents[i].kind` not in `{skill,agent,command}` | 422 | `{"contents[i].kind": "must be one of ..."}` |
| Any `contents[]` slug+kind+version not published in this tenant | 422 | Per-index map flagging each missing row. |
| `(tenant, slug, version)` already exists | 409 | `{"error": "conflict", "message": "plugin <slug>@<version> already exists"}` |

Success: **201 Created** with the full plugin row:

```json
{
  "slug": "rust-axum-toolkit",
  "version": "1.2.0",
  "name": "rust-axum-toolkit",
  "description": "Curated skills, agents, and hooks for Rust + Axum",
  "status": "published",
  "sourcing_mode": "internal",
  "manifest": { "...": "verbatim plugin.json" },
  "contents": [
    { "kind": "skill", "slug": "rust-axum-handler", "version": "1.2.3", "position": 0 }
  ],
  "created_at": "2026-05-24T12:00:00Z",
  "updated_at": "2026-05-24T12:00:00Z"
}
```

Side effects on a successful publish with `status="published"`:

1. **Internal mode** — skill-pool materialises a bare git repo under the
   tenant's storage (`<state-dir>/.../plugins/<slug>.git/`) containing
   the manifest + bundled skill bodies, so `/git/plugins/<slug>.git`
   becomes a valid clone target.
2. **Marketplace entry** — a row is upserted into
   `plugin_marketplace_entries` so the next fetch of
   `/.claude-plugin/marketplace.json` surfaces the plugin. The entry's
   `source.url` points at the tenant's git endpoint for `internal` and
   `mirror`, and at `external_git_url` (with a `github` shortcut when
   applicable) for `external`.

Both side effects are best-effort: a transient failure logs a warning
but does not roll back the publish.

## `GET /v1/plugins` — list

Paginated list of the latest published version per slug. Mirrors
`/v1/skills` semantics — `DISTINCT ON (slug) ORDER BY created_at DESC`,
keyset cursor on `(created_at, id)`.

Query params:

| Param | Type | Description |
|---|---|---|
| `tags` | csv | All tags must be present in `manifest.tags[]`. Plugins whose manifest has no `tags` array never match a tag filter. |
| `status` | string | `draft`, `published` (default), or `archived`. |
| `sourcing_mode` | string | `internal`, `external`, or `mirror`. |
| `limit` | int | Default 50, clamped to 1..200. |
| `cursor` | string | Opaque base64 cursor returned by the previous response. |

Response:

```json
{
  "items": [
    {
      "slug": "rust-axum-toolkit",
      "version": "1.2.0",
      "name": "rust-axum-toolkit",
      "description": "Curated skills, agents, and hooks for Rust + Axum",
      "status": "published",
      "sourcing_mode": "internal",
      "tags": ["rust", "axum"],
      "created_at": "2026-05-24T12:00:00Z"
    }
  ],
  "next_cursor": "MjAyNi0wNS0yNFQxMjowMDowMFp8MDE5MDdkMjItN2Y0..."
}
```

`next_cursor` is only emitted when the response filled the page (`items.length == limit`). A short page is the EOF signal.

## `GET /v1/plugins/{slug}` — latest published

Returns the latest **published** version of `slug` with full
`manifest` + `contents[]`. 404 when no published version exists.

```json
{
  "slug": "rust-axum-toolkit",
  "version": "1.2.0",
  "name": "rust-axum-toolkit",
  "description": "Curated skills, agents, and hooks for Rust + Axum",
  "status": "published",
  "sourcing_mode": "internal",
  "manifest": { "...": "verbatim plugin.json" },
  "contents": [
    { "kind": "skill", "slug": "rust-axum-handler", "version": "1.2.3", "position": 0 }
  ],
  "created_at": "2026-05-24T12:00:00Z",
  "updated_at": "2026-05-24T12:00:00Z"
}
```

## `GET /v1/plugins/{slug}/versions` — version history

Every version of `slug` newest-first, capped at 50 rows. Surfaces all
statuses (`draft`, `published`, `archived`) so curators see archived
rows too.

```json
[
  {
    "version": "1.2.0",
    "status": "published",
    "created_at": "2026-05-24T12:00:00Z",
    "published_by": "platform@acme.example.com"
  },
  {
    "version": "1.1.0",
    "status": "archived",
    "created_at": "2026-04-10T08:00:00Z",
    "published_by": "platform@acme.example.com"
  }
]
```

`published_by` is omitted when `created_by` is NULL. 404 when no row
exists for the slug.

## `DELETE /v1/plugins/{slug}/versions/{version}` — archive

Soft-archive a single version (flip `status` to `archived`).
Idempotent: returns **204 No Content** on first call, **404** thereafter
(already-archived rows are treated as not-found).

```bash
curl -X DELETE \
  -H "Authorization: Bearer $TOKEN" \
  https://acme.skill-pool.example.com/v1/plugins/rust-axum-toolkit/versions/1.1.0
# HTTP/1.1 204 No Content
```

## `POST /v1/plugins/import` — not yet implemented

The CLI's `skill-pool plugin import <git-url>` calls this endpoint and
treats a 404 as a soft "not yet available" (see
`cli/src/client.rs:824-841`, `cli/src/cmd/plugin.rs:261-281`). The
import worker that backs it is tracked separately and will land in a
follow-up issue. Until then, the route returns 404 and the CLI exits
with status 2.

## `GET /.claude-plugin/marketplace.json` — Claude Code marketplace

The catalogue Claude Code consumes via `/plugin marketplace add <url>`.
Public read; **no `Authorization` header** required. Tenant is resolved
from the request's `Host` header (or `X-Skill-Pool-Tenant`), and the
per-tenant rate limiter applies.

Response schema follows the upstream
[Claude Code marketplace spec](https://code.claude.com/docs/en/plugin-marketplaces#marketplace-schema):

```json
{
  "name": "acme",
  "owner": {
    "name": "Acme Inc.",
    "url": "https://acme.skill-pool.example.com/marketplace"
  },
  "plugins": [
    {
      "name": "rust-axum-toolkit",
      "description": "Curated skills, agents, and hooks for Rust + Axum",
      "version": "1.2.0",
      "source": {
        "source": "url",
        "url": "https://acme.skill-pool.example.com/git/plugins/rust-axum-toolkit.git"
      },
      "keywords": ["rust", "axum"]
    }
  ]
}
```

`source` shapes:

| Sourcing mode | `source` shape |
|---|---|
| `internal`, `mirror` | `{"source": "url", "url": "<origin>/git/plugins/<slug>.git"}` (skill-pool hosts the bytes) |
| `external` (github.com top-level repo) | `{"source": "github", "repo": "<owner>/<repo>"}` |
| `external` (other host) | `{"source": "url", "url": "<external_git_url>"}` |

Caching:

- `ETag: "<32-hex>"` — strong, derived from sha256 of the response body.
- `Cache-Control: public, max-age=60` — 60-second TTL matches the
  in-process auth cache so admins see updates within a minute.
- Conditional `GET` with `If-None-Match: "<etag>"` returns **304 Not Modified**.

```bash
curl -i https://acme.skill-pool.example.com/.claude-plugin/marketplace.json
# HTTP/1.1 200 OK
# Content-Type: application/json
# ETag: "a3f1d4..."
# Cache-Control: public, max-age=60
# ...
```

## `/git/plugins/{slug}.git/...` — per-plugin git endpoint

A read-only, hand-rolled smart-HTTP git server scoped to a single
plugin slug. The pair of routes Claude Code's `/plugin install` calls
during a `git clone`:

| Method | Path | Content-Type response |
|---|---|---|
| GET | `/git/plugins/{slug}.git/info/refs?service=git-upload-pack` | `application/x-git-upload-pack-advertisement` |
| POST | `/git/plugins/{slug}.git/git-upload-pack` | `application/x-git-upload-pack-result` |

Scope:

- **Read-only.** `git-receive-pack` is not served — pushes would bypass
  `/v1/plugins` validation.
- **Internal + mirror plugins only.** External-sourced plugins return
  404 here; their bytes live on the curator's upstream host.
- Public read; rate-limited.
- Capability set advertised on the first ref:
  `multi_ack_detailed no-done side-band-64k thin-pack ofs-delta agent=skill-pool/0.1`.

Failure modes:

| Symptom | Cause |
|---|---|
| 400 `unsupported service` | `?service=` is missing or not `git-upload-pack`. |
| 404 | Plugin slug doesn't exist in this tenant, or it's `external`-mode (no local bytes). |
| 404 after a successful publish | Internal git materialisation failed silently (logged warning at publish time). Republish to retry. |

Clone:

```bash
git clone https://acme.skill-pool.example.com/git/plugins/rust-axum-toolkit.git
# Cloning into 'rust-axum-toolkit'...
# remote: ...
```

For the end-to-end install flow Claude Code drives on top of these two
endpoints, see [`docs/wiki/Plugin-Authoring.md`](wiki/Plugin-Authoring.md).
