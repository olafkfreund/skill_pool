# Skill lifecycle (Phase 5)

> Operator-facing reference for how a skill moves from "published" to
> "archived", how usage telemetry is collected, and how dependency
> closures are resolved at install time. Phase 5 is the slice that turns
> the catalog from a write-only registry into a living organism.

This doc ties three Phase 5 sub-systems together:

1. **Decay** — finding and archiving skills nobody uses anymore.
2. **Telemetry** — counting downloads and views without storing user
   content.
3. **Dependencies** — declaring `requires:` in frontmatter, walking the
   closure at install time.

If you only care about one of these, jump to the relevant section. The
[Lifecycle summary](#lifecycle-summary) table at the bottom maps every
verb to the endpoint + table that implements it.

---

## Decay rules

A published skill becomes a **decay candidate** when it stops being
useful. The default heuristic, per the master plan and the
`/v1/tenant/skills/decay` endpoint:

```
candidate ⇔ last_used_at < now() - {days} days
         AND use_count    < {max_uses}
```

The two counters live on the `skills` row itself (migration `0011`):

| column          | type        | bumped by                            |
|-----------------|-------------|--------------------------------------|
| `use_count`     | `INTEGER`   | every `download` or `view` event     |
| `last_used_at`  | `TIMESTAMPTZ` | replaced on every event              |

A `NULL` `last_used_at` (never used) is always treated as a candidate
provided `use_count < max_uses` — that's how a brand-new skill that
sat unused for six months ends up in the graveyard alongside actually
decayed ones.

### Defaults

| knob       | default value | source                                                |
|------------|---------------|-------------------------------------------------------|
| `days`     | `180`         | `DEFAULT_DAYS` in `server/src/routes/decay.rs`         |
| `max_uses` | `3`           | `DEFAULT_MAX_USES` in `server/src/routes/decay.rs`     |
| `limit`    | `200`         | hard-capped at 1000                                   |

Operators can override at query time:

```http
GET /v1/tenant/skills/decay?days=90&max_uses=1&limit=50
Authorization: Bearer <admin-token>
```

### Who archives

Listing decay candidates and flipping them to `archived` is **admin
only** — the endpoints require the `tenant:admin` scope on the API
token (`require_scope` in `decay.rs`). The web catalog renders the
graveyard view from these endpoints; ad-hoc curl works too.

```http
POST /v1/skills/{slug}/archive?kind=skill
Authorization: Bearer <admin-token>
```

Effect: flips the latest published row's `status` from `published` to
`archived`. The list/search endpoints filter `status='published'`, so
the skill disappears from the catalog and from `skill-pool ensure`
output the next time someone re-runs it.

### Reversibility

Archive is **a soft delete**. The row stays in the database, all
referenced dependencies remain intact (`skill_dependencies` rows are
not cascaded), and a future "un-archive" admin endpoint can flip the
status back without losing history. Today there's no UI for that — if
you need to restore, run:

```sql
UPDATE skills SET status = 'published'
WHERE tenant_id = $1 AND slug = $2 AND status = 'archived'
  AND id = (SELECT id FROM skills WHERE tenant_id = $1 AND slug = $2
            ORDER BY created_at DESC LIMIT 1);
```

Future work (see [Future work](#future-work)) replaces this with a
proper admin button.

### Scope: skills only for v1

Decay only applies to `kind = 'skill'` for now. Agents and commands
have different baseline usage patterns (an agent might be invoked
millions of times via tool-call, a slash-command zero times for months
between releases) — the same 180d/3-use heuristic would archive them
incorrectly. Re-tune per-kind decay when agents/commands have enough
traffic to model.

---

## Telemetry policy

Two event kinds are recorded today, defined by the `CHECK` constraint in
migration `0013`:

| event      | trigger                                       | route                                  |
|------------|-----------------------------------------------|----------------------------------------|
| `download` | `GET /v1/skills/{slug}/bundle.tar.gz`          | `skills::get_bundle`                   |
| `view`     | `GET /v1/skills/{slug}/skill-md`               | `skills::get_skill_md`                 |

Both events run through `record_usage` in `server/src/routes/skills.rs`
on a best-effort basis: a DB error logs a warning but never blocks the
user's request.

### What is recorded

Every event row in `skill_usage_events` (migration `0013`) has:

| column      | source                                               |
|-------------|------------------------------------------------------|
| `tenant_id` | resolved tenant from the request                     |
| `skill_id`  | the `skills.id` UUID being fetched                   |
| `event_kind`| `'download'` or `'view'`                              |
| `user_id`   | resolved user (NULL for token-only auth)             |
| `token_id`  | the API token that authenticated the request         |
| `ts`        | server `now()`                                       |

### What is NOT recorded

By design, on purpose:

- **No IP address.** The `audit_events` table records IPs for tenant
  admin actions; usage events are too high-volume to keep IPs at row
  granularity without a privacy review.
- **No user-agent.** Same rationale.
- **No user content / no skill body.** Only the row reference. The
  bundle bytes never end up in this table.
- **No referrer / no session context.** A `view` from the web UI and
  a `view` from a Claude session are indistinguishable in this table.

This is intentional: the table is meant to answer "who used what, when"
at a level coarse enough to make decay decisions and tenant dashboards
without becoming a behavioural-tracking system.

### Aggregations

Two endpoints serve the read side (`server/src/routes/usage.rs`):

| endpoint                          | shape                                                                       |
|-----------------------------------|-----------------------------------------------------------------------------|
| `GET /v1/tenant/usage/timeline`   | per-day `{ day, downloads, views, unique_skills }` (gap-filled w/ zeros)    |
| `GET /v1/tenant/usage/top`        | top N skills in the window: `{ slug, downloads, views, total }`             |

Both are admin-scoped (`tenant:admin`). The timeline query uses
`generate_series` to fill missing days so the dashboard chart has no
gaps; `top` joins back to `skills.slug` so deleted IDs simply drop out
of the result.

### Retention

**No automatic retention today.** Each event becomes one row and stays
there. At our current write rate (single-digit events per skill per
day per tenant) the table can grow for years before needing
partitioning. Once monthly volume crosses ~10M rows the
month-partition migration becomes worthwhile. See
[Future work](#future-work).

For now, operators can trim manually:

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
at publish time. The parsing rule lives in `parse_requires_entry` in
`server/src/routes/skills.rs`:

- `slug` — version range `*` (any).
- `slug@X.Y.Z` — version range is the exact string after `@`.

### The dependency table

Migration `0012_skill_dependencies.sql`:

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
   the target doesn't have to exist yet. Publishing `A` that requires
   not-yet-existing `B` is fine. When `B` is published later, the
   closure resolves cleanly the next time someone walks it.
2. **`ON DELETE CASCADE`** — if the parent skill row is hard-deleted
   (rare; usually archive is enough), the dependency rows go with it.
   Archived skills keep their dependency rows so the graph stays
   intact for forensics.

### The closure endpoint

`GET /v1/skills/{slug}/deps` walks the transitive closure:

```http
GET /v1/skills/axum-tenant-handler/deps

[
  { "slug": "sqlx-migrations", "version_range": "*",     "depth": 1 },
  { "slug": "tenant-ctx",      "version_range": "1.2.0", "depth": 1 },
  { "slug": "logging-init",    "version_range": "*",     "depth": 2 }
]
```

The implementation is a `WITH RECURSIVE` CTE in `routes::skills::get_deps`
that:

- Seeds the closure from `skill_dependencies` rows whose
  `parent_skill_id` matches the resolved slug.
- Joins back through `skills.slug → skill_dependencies.parent_skill_id`
  on each iteration to descend further.
- **Cycle-safe.** `UNION` dedups rows, and a hard `depth < 10` cap is
  belt-and-braces protection against pathological graphs. The depth
  cap means a cycle of two skills requiring each other surfaces as
  finite output, not infinite recursion.
- Returns rows ordered `depth ASC, slug ASC` — the caller can install
  shallow nodes first, or invert the order to install leaves first.

The endpoint is tenant-scoped (closure can only cross edges in the
same tenant) and returns `404` if the parent slug isn't published in
this tenant.

### How `skill-pool ensure` uses it

The CLI's `ensure` command (`cli/src/cmd/ensure.rs`) walks each
manifest entry, calls `/deps`, and builds a deduplicated install plan:

1. For each top-level entry in `[[skills]]`, `[[agents]]`, and
   `[[commands]]`, push it onto the plan.
2. For `[[skills]]` only (agents/commands don't have transitive deps
   today), call `GET /v1/skills/{slug}/deps` and push every entry in
   the closure.
3. **Dedup** by `(slug, kind)` — the same dep pulled by two different
   parents installs exactly once.
4. **Sort deepest-first, then alphabetically.** Leaves land on disk
   before their dependents, which keeps the symlinks coherent if a
   curious user inspects the project mid-install.
5. For each plan entry, resolve `version="*"` against
   `GET /v1/skills/{slug}?kind=...` then download the bundle. A
   missing-from-registry slug (forward reference not yet published)
   logs `warn: skipping …` and the rest of the plan continues — the
   user can re-run `ensure` after the missing piece is published.

The CLI uses the depth cap implicitly: any closure deeper than ten
levels is malformed and gets capped server-side, so `ensure` will
never spin forever on a corrupt graph.

---

## Lifecycle summary

The table below maps every verb a skill goes through to the endpoint
that handles it and the table(s) it touches. Use this as a quick
reference when wiring up dashboards or writing runbook steps.

| verb                  | endpoint                                                       | tables touched                          |
|-----------------------|----------------------------------------------------------------|------------------------------------------|
| **publish**           | `POST /v1/skills` (multipart)                                  | `skills`, `skill_dependencies`           |
| **embed**             | (inline, during publish, if `--features fastembed`)            | `skills.description_embedding`           |
| **list / search**     | `GET /v1/skills?query=&tags=&semantic=&kind=`                  | `skills` (read)                          |
| **fetch metadata**    | `GET /v1/skills/{slug}?kind=`                                  | `skills` (read)                          |
| **fetch body**        | `GET /v1/skills/{slug}/skill-md?kind=`                         | `skills` (read), `skill_usage_events`    |
| **download bundle**   | `GET /v1/skills/{slug}/bundle.tar.gz?kind=`                    | `skills` (read), `skill_usage_events`    |
| **install via MCP**   | `tools/call install_skill { slug, kind }`                      | `skills` (read)                          |
| **bump use_count**    | (inline, during fetch body / download)                         | `skills.use_count`, `skills.last_used_at`|
| **walk closure**      | `GET /v1/skills/{slug}/deps`                                   | `skill_dependencies` (recursive)         |
| **decay candidate**   | `GET /v1/tenant/skills/decay?days=&max_uses=`                  | `skills` (read)                          |
| **archive**           | `POST /v1/skills/{slug}/archive`                               | `skills.status`, `audit_events`          |
| **usage timeline**    | `GET /v1/tenant/usage/timeline?days=`                          | `skill_usage_events` (read)              |
| **top skills**        | `GET /v1/tenant/usage/top?days=&limit=`                        | `skill_usage_events`, `skills` (read)    |

Defaults that operators most commonly tune: decay `days=180`,
`max_uses=3`; usage timeline `days=30`; closure depth cap `10`.

---

## Future work

Phase 5 ships the bones. The following deferred items live on the
roadmap; tracking them here so the next operator who edits this doc
has the context.

- **Sliding-window decay.** Today decay is binary: "below max_uses in
  the last N days". A sliding window (e.g. "downloads in last 14 days
  must exceed downloads in same window 6 months ago") would catch
  declining skills before they hit the hard threshold. Requires
  back-fill of `skill_usage_events` partitions and a heavier query.
- **Retention rules on `skill_usage_events`.** No automatic cleanup
  today. Once the table volume warrants it, partition monthly and
  drop partitions older than the tenant's configured retention
  window. Surface as a per-tenant setting (`usage_retention_days`).
- **Dependency conflict detection across versions.** Today
  `version_range` is opaque text. A future slice parses semver and
  flags incompatible closures (e.g. parent A requires `B@1.x` while
  parent C requires `B@2.x` and both are pulled into the same plan).
  Right now the CLI installs both versions side-by-side under
  `~/.skill-pool/library/<tenant>/<slug>@<version>/`, which usually
  works but can confuse `.claude/skills/<slug>` symlink ownership.
- **Per-kind decay tuning.** As noted above, agents and commands need
  their own thresholds before they participate in the graveyard view.
- **Un-archive UI button.** Today archive is one-way through the UI;
  reversal requires a SQL update. Add a `POST /v1/skills/{slug}/unarchive`
  with admin scope + audit event.
- **Closure caching.** The recursive CTE is cheap today but caching
  the closure (with `created_at` invalidation on dep publish) saves
  round-trips for the hot path of `ensure` against deeply nested
  graphs.
