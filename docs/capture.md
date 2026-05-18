# Retrospective capture (Phase 4 — first slice)

When you solve a non-trivial bug or learn a new pattern, the hard-won
insight should land in the team's skill catalog without you stopping to
fill out a publish form. Phase 4 is the path from "I figured it out" to
"published team skill".

This doc covers the **explicit path** — the first slice. The async
signal scorer + capturer daemon land in Phase 4.5; the architecture
section of the master plan describes both layers.

## The explicit path today

```
  developer       │   CLI                │   server               │   curator
  ────────        │   ────                │   ──────                │   ────────
  finished a      │   skill-pool capture │   POST /v1/drafts       │
  task, drafted   │     ./lesson-foo     │     (multipart bundle)  │
  a SKILL.md      │                       │   → status=pending      │
                   │                       │                          │
                                          │                          │   reviews
                                          │   POST /v1/drafts/{id}  │   in the
                                          │     /publish            │←──  web UI,
                                          │   → INSERT skills       │   assigns
                                          │                          │   version
```

## Capturing from the CLI

```bash
# In a project where you've just solved something:
mkdir lesson-axum-handler
cat > lesson-axum-handler/SKILL.md <<'MD'
---
name: axum-handler-tip
description: Pattern for tenant-scoped axum extractors that avoids the
  borrow-checker dance with a request-scoped clone.
when_to_use: When building Axum handlers that need TenantCtx + AppState.
tags: [rust, axum, tenant]
---

# axum-handler-tip

The pattern is …
MD

skill-pool capture ./lesson-axum-handler \
  --notes "Found while fixing the SCIM list endpoint — see PR #42"
```

What lands in the curator inbox:
- The bundle (server validates frontmatter + scans for secrets first).
- Status `pending`, origin `cli`.
- Tags merged from frontmatter + `--tags` flag.
- Free-form `--notes` for "why this matters" context.

Drafts are **tenant-scoped** — only the tenant you authenticated against
sees them.

## Reviewing in the web UI

Navigate to `/drafts` in the portal. The inbox shows pending drafts with:
- One-click **Publish** — assigns a version (the curator types it),
  promotes to `skills`, the bundle moves to the canonical key.
- One-click **Discard** — marks the draft as `discarded` (kept for
  telemetry, hidden from the default view).
- Filter tabs for `Pending` / `Published` / `Discarded` / `All`.

Publishing in one transaction:
1. Copies the bundle to the canonical `<tenant>/<slug>/<version>.tar.gz`.
2. INSERTs into `skills` (rolls back if `(tenant, slug, version)` collides).
3. UPDATEs the draft to `status='published'` with the new `skill_id` /
   `published_version`.

Re-publishing the same draft 400s. Re-using `(slug, version)` against an
already-published skill 400s with a "pick a different version" message.

## API contract

```
POST   /v1/drafts                  multipart: metadata JSON + bundle .tar.gz
                                   → 201 { id, slug, status, ... }

GET    /v1/drafts?status=pending   → [Draft, …]
       (also: published, discarded, all)

GET    /v1/drafts/{id}             → Draft
GET    /v1/drafts/{id}/skill-md    → text/plain SKILL.md from bundle

POST   /v1/drafts/{id}/publish     { version: "1.0.0", slug?: "override" }
                                   → { draft_id, skill_id, slug, version }

POST   /v1/drafts/{id}/discard     → 204 No Content
```

All endpoints require the `skills:read` scope for GET, `skills:publish`
for POST. Drafts are tenant-isolated via the standard `TenantCtx`
extractor.

## Storage layout

Drafts live under a separate object-storage prefix so that:
- A discarded draft is a single DELETE + a single object purge.
- Publishing copies the bytes into the canonical skill key — no
  versioning collisions with active publishes.

```
{tenant_id}/drafts/{draft_uuid}.tar.gz      ← while pending
{tenant_id}/{slug}/{version}.tar.gz          ← after publish
```

## What's NOT yet wired (Phase 4.5+)

- **Signal scorer** — runs in a `Stop` hook every assistant turn, scores
  the session against explicit markers / failing→passing test
  transitions / multiple-retry patterns. Auto-fires the capturer when
  the score crosses a threshold. Off by default in v1.
- **`skill-pool-capturer` daemon** — systemd user unit that pulls
  high-score sessions, runs a two-stage LLM pipeline (Haiku extractor →
  Sonnet drafter), and POSTs to `/v1/drafts` with origin
  `capture-scorer`.
- **Embedding dedup** — before insert, check the new draft's
  description against existing skills. If `cosine > 0.85`, file as a
  "merge proposal" instead of a fresh draft.
- **Curator notifications** — desktop / email "N drafts ready for
  review" pings when the inbox grows.

The first slice ships the human-in-the-loop path with no LLM work and no
in-session prompts — exactly the "explicit path stable before scorer"
policy in the master plan.

## Audit trail

Every mutating draft endpoint writes to `audit_events`:
- `draft.create` (with size, sha256, slug)
- `draft.publish` (with version, target skill_id)
- `draft.discard`

Append-only, retained per-tenant policy. Same export pipeline as the
rest of the audit log.
