# Phase 5 — Skill Lifecycle

> How a skill moves from `published` to `archived`, how usage
> telemetry is collected, how dependency closures resolve at install
> time, and how the MCP and git-mirror seams expose all of it.

Three Phase 5 sub-systems tie this page together:

1. **Decay** — finding and archiving unused skills.
2. **Telemetry** — counting downloads and views without storing user
   content.
3. **Dependencies** — declaring `requires:` in frontmatter, walking
   the closure at install time.

If you only care about one of these, jump to the relevant section.
The [Lifecycle summary table](#lifecycle-summary) at the bottom maps
every verb to its endpoint + table.

---

## Decay rules

A published skill becomes a **decay candidate** when it stops being
useful:

```text
candidate ⇔ last_used_at < now() - {days} days
         AND use_count    < {max_uses}
```

The two counters live on the `skills` row itself (migration `0011`):

| column         | type          | bumped by                            |
|----------------|---------------|--------------------------------------|
| `use_count`    | `INTEGER`     | every `download` or `view` event     |
| `last_used_at` | `TIMESTAMPTZ` | replaced on every event              |

A `NULL` `last_used_at` (never used) is always treated as a candidate
provided `use_count < max_uses` — that's how a brand-new skill that
sat unused for six months ends up in the graveyard alongside actually
decayed ones.

### Defaults

| knob       | default | source                                        |
|------------|---------|-----------------------------------------------|
| `days`     | 180     | `DEFAULT_DAYS` in `server/src/routes/decay.rs`|
| `max_uses` | 3       | `DEFAULT_MAX_USES` in `server/src/routes/decay.rs`|
| `limit`    | 200     | hard-capped at 1000                           |

### Background sweep

A background tokio task (`spawn_decay_sweep` in `server/src/main.rs`)
runs the same heuristic periodically and flips qualifying rows to
`status = 'archive_candidate'` so curators see them flagged
proactively — without auto-archiving anything. Archive remains an
explicit admin verb.

| knob                          | default      | source                                          |
|-------------------------------|--------------|-------------------------------------------------|
| `decay_check_interval_secs`   | `86400` (24h)| `SKILL_POOL_DECAY_CHECK_INTERVAL_SECS`          |
| stale-days threshold          | 180          | `routes::decay::DEFAULT_SWEEP_STALE_DAYS`       |
| min-uses threshold            | 3            | `routes::decay::DEFAULT_SWEEP_MIN_USES`         |

Set the interval to `0` to disable the sweep (the on-demand
`/v1/tenant/skills/decay` endpoint continues to work). The sweep
shares the queue worker's shutdown channel so SIGTERM drains both at
the same time. Errors log + continue: a transient DB blip never
crashes the server.

`archive_candidate` lives in `skills.status` (migration `0027`).
Catalog list/search filters `status = 'published'`, so flagged
skills disappear from the catalog automatically.

### Listing candidates

```http
GET /v1/tenant/skills/decay?days=90&max_uses=1&limit=50
Authorization: Bearer <admin-token>
```

Admin-only (`tenant:admin` scope). Sorted by `last_used_at ASC` so
the stalest rows surface first.

### Archiving

```http
POST /v1/skills/{slug}/archive?kind=skill
Authorization: Bearer <admin-token>
```

Flips the latest published row's `status` from `published` to
`archived`. The skill disappears from the catalog and from
`skill-pool ensure` output the next time anyone re-runs it.

### Reversibility

Archive is **soft delete**. Row stays in the DB; `skill_dependencies`
rows are not cascaded; a future un-archive admin endpoint can flip
status back without losing history. Today there's no UI for that —
manual restore:

```sql
UPDATE skills SET status = 'published'
WHERE tenant_id = $1 AND slug = $2 AND status = 'archived'
  AND id = (SELECT id FROM skills WHERE tenant_id = $1 AND slug = $2
            ORDER BY created_at DESC LIMIT 1);
```

### Scope

Decay applies to `kind = 'skill'` only for v1. Agents and commands
have different baseline usage patterns; re-tune per-kind decay when
they have enough traffic to model.

---

## Telemetry policy

Two event kinds, defined by the `CHECK` constraint in migration `0013`:

| event      | trigger                                       | route                                  |
|------------|-----------------------------------------------|----------------------------------------|
| `download` | `GET /v1/skills/{slug}/bundle.tar.gz`          | `skills::get_bundle`                   |
| `view`     | `GET /v1/skills/{slug}/skill-md`               | `skills::get_skill_md`                 |

Both run through `record_usage` in `server/src/routes/skills.rs` on a
best-effort basis: a DB error logs a warning but never blocks the
user's request.

### What is recorded

| column      | source                                            |
|-------------|---------------------------------------------------|
| `tenant_id` | resolved tenant from the request                  |
| `skill_id`  | the `skills.id` UUID being fetched                 |
| `event_kind`| `'download'` or `'view'`                           |
| `user_id`   | resolved user (NULL for token-only auth)          |
| `token_id`  | the API token that authenticated the request      |
| `ts`        | server `now()`                                    |

### What is NOT recorded

By design:

- **No IP address.** The `audit_events` table records IPs for tenant
  admin actions; usage events are too high-volume for IP-granular
  storage without a privacy review.
- **No user-agent.** Same rationale.
- **No user content / no skill body.** Only the row reference.
- **No referrer / no session context.** A view from the web UI and a
  view from a Claude session are indistinguishable.

This is intentional: the table answers "who used what, when" at a
level coarse enough to make decay decisions and tenant dashboards
without becoming a behavioural-tracking system.

### Aggregations

| endpoint                          | shape                                                            |
|-----------------------------------|------------------------------------------------------------------|
| `GET /v1/tenant/usage/timeline`   | per-day `{ day, downloads, views, unique_skills }` (gap-filled)  |
| `GET /v1/tenant/usage/top`        | top N skills: `{ slug, downloads, views, total }`                |
| `POST /v1/usage`                  | CLI-driven `view` event; body: `{ skill_id, kind, event, project_hash }` |

### CLI usage events on session-load (#7)

`skill-pool ensure` POSTs one `view` event per successful skill
install to `/v1/usage` so the decay model sees session-load activity
alongside actual bundle downloads. Otherwise a popular skill that
gets installed once and read from disk many times looks unused from
the registry's vantage point.

Defaults:

- **Telemetry is ON by default.** The CLI already authenticates with
  its API token; sending one `view` event per install is symmetrical
  with that trust.
- **`--no-telemetry`** opts out per invocation.
- Fire-and-forget — a network blip logs at `debug` and never blocks
  the install.

`project_hash` is the SHA-256 of the project root, truncated to 16
hex chars. It anonymizes which project on which machine sent the
event so the server can dedup repeats without persisting a
reversible identifier.

### Retention

**No automatic retention today.** At current write rate (single-digit
events per skill per day per tenant) the table can grow for years
before needing partitioning. Once monthly volume crosses ~10M rows
the month-partition migration becomes worthwhile.

Operators can trim manually:

```sql
DELETE FROM skill_usage_events
WHERE tenant_id = $1 AND ts < now() - INTERVAL '2 years';
```

The aggregation endpoints recompute on every call, so trimming is
safe.

---

## Dependency resolution

A skill declares dependencies via `requires:` in its SKILL.md
frontmatter:

```yaml
---
name: axum-tenant-handler
description: ...
requires:
  - sqlx-migrations
  - tenant-ctx@1.2.0
---
```

Each entry becomes one row in `skill_dependencies` (migration `0012`)
at publish time. Parsing rule (`parse_requires_entry`):

- `slug` — version range `*` (any).
- `slug@X.Y.Z` — version range is the exact string after `@`.

### The dependency table

```sql
CREATE TABLE skill_dependencies (
    id              UUID  PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID  NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    parent_skill_id UUID  NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    requires_slug   TEXT  NOT NULL,
    version_range   TEXT  NOT NULL DEFAULT '*',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (parent_skill_id, requires_slug)
);
```

Two important properties:

1. **Forward references are legal.** `requires_slug` is plain text —
   the target doesn't have to exist yet. When the target is published
   later, the closure resolves cleanly on the next walk.
2. **`ON DELETE CASCADE`** — if the parent is hard-deleted, deps go
   with it. Archived skills keep their dependency rows so the graph
   stays intact for forensics.

### The closure endpoint

`GET /v1/skills/{slug}/deps` walks the transitive closure. The
implementation is a `WITH RECURSIVE` CTE in
`routes::skills::get_deps`:

- Seeds from `skill_dependencies` rows where `parent_skill_id` matches.
- Joins back through `skills.slug → skill_dependencies.parent_skill_id`
  on each iteration.
- **Cycle-safe.** UNION dedups; a hard `depth < 10` cap is
  belt-and-braces against pathological graphs.
- Returns rows ordered `depth ASC, slug ASC`.

Tenant-scoped (closure can only cross edges in the same tenant);
404 if the parent slug isn't published in this tenant.

### Version-range conflict detection (#7)

At publish time, every entry in `requires:` is checked against
existing dependency rows in the same tenant. If another published
skill already requires the same target slug at an *incompatible*
version range, publish fails with `409 Conflict`:

```text
skill `b` requires `lib@2.0.0` but skill `a` already requires `lib@1.0.0`
```

v1 compatibility predicate (`check_version_compatibility`):

| left range | right range | compatible? |
|------------|-------------|-------------|
| `*`        | anything    | yes         |
| anything   | `*`         | yes         |
| `1.0.0`    | `1.0.0`     | yes (identical) |
| `1.0.0`    | `2.0.0`     | **no** → 409 |
| `^1.2`     | `^1.3`      | **no** → 409 (opaque string compare) |

For v1 we deliberately do not parse semver. Anything beyond `*` and
exact versions is treated as an opaque string. False-positives push
operators to align ranges explicitly — the right outcome until we
ship a proper resolver.

### How `skill-pool ensure` uses it

`cli/src/cmd/ensure.rs` walks each manifest entry, calls `/deps`, and
builds a deduplicated install plan:

1. For each top-level entry in `[[skills]]`, `[[agents]]`,
   `[[commands]]`, push it onto the plan.
2. For `[[skills]]` only, call `GET /v1/skills/{slug}/deps` and push
   every entry in the closure.
3. **Dedup** by `(slug, kind)` — the same dep pulled by two parents
   installs exactly once.
4. **Sort deepest-first, then alphabetically.** Leaves land on disk
   before their dependents.
5. For each plan entry, resolve `version="*"` against
   `GET /v1/skills/{slug}?kind=...` then download. A
   missing-from-registry slug (forward reference) logs `warn:
   skipping …` and the rest of the plan continues.

---

## Agents and commands

The `kind` discriminator on the same catalog table holds skills,
Claude Code subagents (`agent`), and slash-commands (`command`). All
three share the schema, validation, dependency graph, embedding
column, and decay model.

CLI verbs:

- `skill-pool add-agent <slug>` — `kind=agent`, appends to
  `[[agents]]` array.
- `skill-pool add-command <slug>` — `kind=command`, appends to
  `[[commands]]` array.
- `skill-pool publish ./dir --kind agent --version 0.1.0`

Catalog endpoints accept the same `?kind=` query param; default is
`skill`.

Decay candidates and the MCP `search_skills` adapter are
**skills-only** for v1 — agents/commands have different baseline
usage patterns.

---

## MCP `install_skill` tool

Beyond `search_skills` and `get_skill` (see [MCP Integration](MCP-Integration.md)),
the `tools/call install_skill` action lets a Claude session install a
catalog entry without leaving the conversation. Same bundle download
path as `skill-pool ensure`; tenant-scoped; returns content blocks
with the install location.

---

## Optional git mirror

Postgres is the source of truth for the catalog. For teams that want
a human-readable, audit-grade history on disk, the server can
additionally commit every successful publish into a Git repo.

Enable with a single env var:

```bash
SKILL_POOL_GIT_REPO_PATH=/var/lib/skill-pool/catalog-mirror
```

When set, both publish paths (`POST /v1/skills` and the
`POST /v1/drafts/{id}/publish` promotion) spawn a detached
`git_sync::commit_skill` task after a successful row insert. The
publish response is **never** blocked on the Git side — if `git`
isn't installed, the repo path doesn't exist, or the commit fails for
any reason, the failure is logged and the publish still returns 2xx.

On-disk layout:

```text
<repo>/<tenant_slug>/<kind>/<slug>/<version>/SKILL.md
                                              <other-bundle-files>
```

`<kind>` is one of `skill`, `agent`, `command`. Promoted drafts
always write under `skill/`. Each commit subject is
`publish: <tenant>/<kind>/<slug>@<version>` and is authored as
`skill-pool@local`.

For signed commits or a custom author, run the path under a working
tree whose `.git/config` already sets those — the spawned process
picks up local config via `git -C <repo>`.

---

## Lifecycle summary

| verb                  | endpoint                                                       | tables touched                          |
|-----------------------|----------------------------------------------------------------|------------------------------------------|
| **publish**           | `POST /v1/skills` (multipart)                                  | `skills`, `skill_dependencies`           |
| **embed**             | (inline, during publish, if `--features fastembed`)            | `skills.description_embedding`           |
| **list / search**     | `GET /v1/skills?query=&tags=&semantic=&kind=`                  | `skills` (read)                          |
| **fetch metadata**    | `GET /v1/skills/{slug}?kind=`                                  | `skills` (read)                          |
| **fetch body**        | `GET /v1/skills/{slug}/skill-md?kind=`                         | `skills`, `skill_usage_events`           |
| **download bundle**   | `GET /v1/skills/{slug}/bundle.tar.gz?kind=`                    | `skills`, `skill_usage_events`           |
| **install via MCP**   | `tools/call install_skill { slug, kind }`                      | `skills` (read)                          |
| **bump use_count**    | (inline, during fetch body / download)                         | `skills.use_count`, `skills.last_used_at`|
| **walk closure**      | `GET /v1/skills/{slug}/deps`                                   | `skill_dependencies` (recursive)         |
| **decay candidate**   | `GET /v1/tenant/skills/decay?days=&max_uses=`                  | `skills` (read)                          |
| **decay sweep**       | background tokio task (every `decay_check_interval_secs`)      | `skills.status` ← `archive_candidate`    |
| **archive**           | `POST /v1/skills/{slug}/archive`                               | `skills.status`, `audit_events`          |
| **usage timeline**    | `GET /v1/tenant/usage/timeline?days=`                          | `skill_usage_events` (read)              |
| **top skills**        | `GET /v1/tenant/usage/top?days=&limit=`                        | `skill_usage_events`, `skills` (read)    |
| **CLI usage event**   | `POST /v1/usage`                                                | `skill_usage_events`, `skills`           |

Defaults operators most commonly tune: decay `days=180`,
`max_uses=3`; usage timeline `days=30`; closure depth cap `10`.

---

## Future work

- **Sliding-window decay.** Replace binary "below max_uses in last N
  days" with "downloads in last 14 days must exceed downloads in the
  same window 6 months ago".
- **Retention rules on `skill_usage_events`.** Partition monthly once
  volume warrants it; expose `usage_retention_days` per tenant.
- **Semver-aware conflict detection.** Parse semver ranges properly;
  downgrade `^1.2`/`^1.3` false-positives to "OK".
- **Per-kind decay tuning.** Tune agents/commands separately once they
  have enough traffic.
- **Un-archive UI button.** `POST /v1/skills/{slug}/unarchive` with
  admin scope + audit event.
- **Closure caching.** Cache the closure with `created_at`
  invalidation on dep publish for the hot `ensure` path.

---

## Where to read next

- [API Reference](API-Reference.md#skills-catalog) — endpoint shapes
- [CLI Reference](CLI-Reference.md) — `add-agent`, `add-command`, `ensure`
- [MCP Integration](MCP-Integration.md) — Claude as a catalog client
- [Phase 4 — Capture](Phase-4-Capture.md) — how drafts get into the catalog

## Cross-links into the codebase

- `server/src/routes/decay.rs` — decay endpoint + sweep
- `server/src/routes/skills.rs` — publish, fetch, dep parsing
- `server/src/routes/usage.rs` — telemetry aggregations
- `server/src/git_sync.rs` — best-effort git mirror
- `server/src/embedding.rs` — pgvector + fastembed
- `server/migrations/0011_skill_use_count.sql`
- `server/migrations/0012_skill_dependencies.sql`
- `server/migrations/0013_skill_usage_events.sql`
- `server/migrations/0027_skills_status_archive_candidate.sql`
- `cli/src/cmd/ensure.rs` — closure walk + dedup
- `docs/lifecycle.md` — original lifecycle note this page mirrors
