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

## Signal scorer (Phase 4.5 — wired today)

The scorer is a `Stop`-hook that fires after every assistant turn,
reads the session transcript, and persists a deterministic score to
`~/.skill-pool/sessions/<session_id>.json`. **No LLM. No network. No
mid-session prompts.** Designed to run in well under 50 ms.

### Install

```bash
skill-pool hook-install --with-scorer
```

This installs three hooks:
- `SessionStart` → `skill-pool ensure --quiet` (Phase 3)
- `Stop`         → `skill-pool capture-score`  (Phase 4.5, per-turn)
- `SessionEnd`   → `skill-pool capture-queue`  (Phase 4, once at end)

`--remove` pulls all three. The CLI preserves any other hooks in
`.claude/settings.json` — both install and remove operate on a JSON
merge, never an overwrite.

### SessionEnd queueing

Where the Stop hook re-scores every turn (cheap, deterministic), the
SessionEnd hook fires exactly once when a session terminates. It reads
the score file that the Stop hook wrote and, if the total is at or
above the configured threshold, drops a small marker into the queue
directory:

```
~/.skill-pool/queue/<session_id>.queued   ← {"queued_at", "session_id", "score", "threshold"}
```

The Phase 4.6 capturer daemon consumes these markers. Until the daemon
lands the existing hourly `skill-pool capture-run` keeps working off
score files directly — the queue is additive, not a replacement.

Threshold precedence:
1. `--threshold N` flag on the hook command
2. `SKILL_POOL_CAPTURE_THRESHOLD` env var
3. Default: **50** (deliberately lower than the per-turn scorer's
   draft threshold of 100; SessionEnd fires only once, so we surface
   more sessions to the LLM gate downstream).

A non-numeric env value falls back to the default with a `tracing`
warning rather than failing the hook — same fail-soft policy as
`capture-score`.

### Signals scored today

| Rule                        | Weight | Threshold                                       |
| --------------------------- | -----: | ----------------------------------------------- |
| Explicit marker             |   1000 | user said "remember this" / "TIL" / "/capture-skill" |
| Failing → passing test recovery | 50 | same `cargo test`/`pytest`/`npm test` failed ≥2× then passed |
| Edit retries on one file    |     30 | >3 failed `Edit`/`Write` on the same `file_path` |
| Cross-session recurrence    |     30 | same fingerprint (first 2 non-flag tokens of a failed Bash, or failed Edit basename) seen in 3+ distinct local sessions |
| Novel command               |     15 | failed Bash stem not present in `~/.bash_history` / `~/.zsh_history` (per distinct stem, capped at 3) |
| Long session                |      5 | >20 assistant turns                              |

The recurrence index lives at `~/.skill-pool/recurrence_index.json` and
maps fingerprint → `[session_ids]`. Each `capture-score` invocation
appends the current session before consulting the count, so the same
session never inflates its own recurrence score.

Default draft-worthy threshold: **score ≥ 100**. The capturer daemon
(Phase 4.6) will pick from `sessions/` files at or above this; for now
the threshold drives the ★ marker in `capture-status`.

### Inspect

```bash
skill-pool capture-status
# 12 sessions scored (3 ≥ draft threshold of 100)
#
#   SCORE TURNS          CWD                                      SESSION
#  ★1050  3              /proj/foo                                signals-1
#         ↳ explicit_marker: user said `remember this`
#  ★ 130  18             /proj/bar                                a4b2c1d…
#         ↳ test_recovery: `cargo test` failed 3× then passed
#     5   26             /proj/baz                                f8e9d2c…
#         ↳ long_session: 26 assistant turns in this session
```

`--json` dumps the raw records — useful for piping into the capturer
daemon when it lands.

### Scorer signals — all five wired today

The full set from the master plan is now scoring. Cross-session
recurrence and novel-command both read state outside the transcript
(the local recurrence index and the user's shell history respectively)
but neither makes a network call — the hook stays well under the 50ms
budget.

## Capturer pipeline (Phase 4.6 — wired today)

The capturer is the LLM layer above the scorer. It turns "this session
was worth saving" into "a draft is in the inbox" without anyone typing
`skill-pool capture` by hand. Cron-driven, idempotent, two-stage so
that ~70% of sessions cost only the cheap extractor pass.

### Run

```bash
skill-pool capture-run                # process up to 5 sessions
skill-pool capture-run --limit 20     # cost cap per pass
skill-pool capture-run --dry-run      # show what would happen
skill-pool capture-run --stage1-model claude-haiku-4-5-20251001 \
                      --stage2-model claude-sonnet-4-6
```

### Pipeline

```
  for each session in ~/.skill-pool/sessions/ where
        score >= 100 AND capture_state is None:
    1. read transcript from ~/.claude/projects/.../session.jsonl
    2. Stage 1 — Haiku — returns JSON:
         { problem, solution_steps, generalizable, scope, preconditions }
    3. if generalizable == false → state.stage = Stage1Rejected, STOP
    4. Stage 2 — Sonnet — returns SKILL.md
    5. client-side validate (frontmatter, secret scan, /home/ paths)
    6. tar.gz + POST /v1/drafts with origin=capture-scorer
    7. persist updated capture_state
```

State transitions land in the score record so the next pass skips
already-processed sessions:

| `capture_state.stage`   | What it means                                          |
|-------------------------|--------------------------------------------------------|
| `stage1_rejected`       | Stage 1 said `generalizable: false`. No draft.         |
| `stage1_parse_failure`  | Stage 1 JSON didn't parse. Future run may retry.       |
| `stage2_rejected`       | Stage 2's SKILL.md failed client-side validation.      |
| `drafted`               | Successfully POSTed to `/v1/drafts`. Inbox now has it. |
| `server_rejected`       | Server returned non-2xx (e.g. dedupe / network).       |

### Scheduling

Install the systemd user unit + timer (hourly with jitter):

```bash
cp packaging/systemd/skill-pool-capturer.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now skill-pool-capturer.timer
```

See `packaging/systemd/README.md` for full instructions.

### Required environment

- `ANTHROPIC_API_KEY` — for the Messages API. The capturer fails fast
  and tells you to set it.
- `SKILL_POOL_REGISTRY` (or a config file from `skill-pool login`) — for
  the draft POST.

### Cost shape

Stage 1 is **Haiku** with `max_tokens=1024` and `temperature=0`. The
prompt is ~500 tokens of system text + a trimmed transcript capped at
12000 chars (~3000 input tokens). One pass per session is a few cents
worst-case, fractions thereof typically.

Stage 2 (**Sonnet**, `max_tokens=2048`, `temperature=0.2`) only runs on
sessions Stage 1 approved. The master plan estimates ~30% pass-through
rate — meaning the expensive call is paid only on the small fraction
worth drafting.

## Embedding dedup (Phase 5 — wired today)

The capturer is good at producing drafts, but a real team will quickly
generate near-duplicates ("axum middleware tip" published in March
becomes "axum middleware pattern" captured in May). The server runs an
embedding-based dedup pass on every draft create so the curator can
*merge* instead of stockpiling variants.

### How it works

On `POST /v1/skills` (publish) the server computes a 384-dim embedding
of the description and persists it in `skills.description_embedding`
(`vector(384)` column via pgvector).

On `POST /v1/drafts` (create) the server:

1. Computes the embedding of the new description.
2. Queries existing published skills in the same tenant:
   `1 - (description_embedding <=> $new_embedding) AS similarity`
   ordered by closeness, limit 1.
3. If `similarity >= 0.85`, persists `merge_proposal_skill_id` +
   `merge_proposal_similarity` on the draft row.
4. The response (and `GET /v1/drafts`) surface
   `merge_proposal_slug` + `merge_proposal_similarity`.

The web inbox shows a "Looks like *foo* (94% match)" badge with a link
to the proposed target skill.

### Tenant isolation

Dedup queries are scoped to the same tenant. A near-duplicate in
another tenant's catalog never flags — confirmed by the integration
test.

### Build switch

Embedding is gated behind the `fastembed` Cargo feature so default
builds (and CI) don't pull in ONNX Runtime or HuggingFace network:

```bash
# Default build — schema columns exist, dedup is a no-op:
cargo build -p skill-pool-server

# Embedding-enabled build:
cargo build -p skill-pool-server --features fastembed
```

With the feature on AND `embedding.enabled = true` in config, the
`FastembedEmbedder` lazy-loads `bge-small-en-v1.5` (~30MB) on first
use. Without the feature, the server runs `NullEmbedder` and the
embedding columns stay NULL — schema and code degrade gracefully.

### Pluggable embedders

The `Embedder` trait in `server/src/embedding.rs` is the seam. Adding
another provider (Voyage AI, OpenAI text-embedding-3, a fine-tuned
in-house model) is a new impl + a config switch; the schema stays put
because everything goes through `vector_to_pg_literal`.

### Backfill for pre-Phase-5 skills

Skills published before the embedding column existed have
`description_embedding IS NULL`. Operators backfill via the server CLI:

```bash
# Dry-run a single tenant first.
skill-pool-server admin backfill-embeddings --tenant acme --limit 50 --dry-run

# Real run, all tenants.
skill-pool-server admin backfill-embeddings --limit 5000
```

Idempotent: only NULL rows are touched. Streams in 50-row pages so a
large catalog can be processed without holding the whole result set in
memory. The command fails fast if the server was built without
`--features fastembed` (or another Embedder is wired).

### Curator notifications (Phase 5 — wired today)

Per-tenant webhook fires fire-and-forget on every `draft.create`. Compatible
with Slack/Discord incoming webhooks out of the box — the payload has a
top-level `text` field plus structured `event`/`tenant`/`draft` fields
for custom receivers.

Configure once via the admin portal at `/admin/notifications` (or via
`PUT /v1/tenant/notifications`):

```http
PUT /v1/tenant/notifications
Authorization: Bearer <admin-token>
Content-Type: application/json

{ "webhook_url": "https://hooks.slack.com/...", "webhook_secret": "optional" }
```

When a secret is configured the server signs each delivery with
HMAC-SHA256 and ships the digest in `X-Skill-Pool-Signature: sha256=<hex>`
— same convention as GitHub/Stripe webhooks.

Delivery semantics:
- Runs on a detached `tokio::spawn` so the original `POST /v1/drafts`
  returns immediately.
- 5s timeout per attempt, one retry on transient failure (network/5xx).
  4xx responses are treated as permanent (likely misconfiguration).
- Every attempt — success or failure — writes to `audit_events` with
  action `notification.deliver`.

Sidebar badge: the web layout polls `GET /v1/tenant/notifications/pending-count`
on every page load and renders a primary-colored pill next to "Drafts"
showing the count of pending drafts.

### What's still NOT wired (Phase 5+)

- **Email notifications** — needs SMTP config + templates + deliverability.
  Webhook + Slack/Discord covers most teams; email is the next layer.
- **Cross-session recurrence + novel-command signals** — need
  persisted historical state (across sessions and shell history).
- **NixOS module** — declarative `services.skill-pool-capturer.enable`
  instead of the manual unit-copy step.

The signal scorer plus the two-stage drafter together give the policy
the master plan called for: precision over recall, deterministic gate
first, LLM only on the fraction that clears it, human-in-the-loop on
every published draft.

## Audit trail

Every mutating draft endpoint writes to `audit_events`:
- `draft.create` (with size, sha256, slug)
- `draft.publish` (with version, target skill_id)
- `draft.discard`

Append-only, retained per-tenant policy. Same export pipeline as the
rest of the audit log.
