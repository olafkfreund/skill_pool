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
