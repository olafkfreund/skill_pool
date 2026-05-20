# CLI Reference

> Every `skill-pool` subcommand, the full set of flags, environment
> variables, and a 1–2 line example each. Generated from
> `cli/src/main.rs` (the `clap` enum is the source of truth) and the
> per-command implementations under `cli/src/cmd/`.

The CLI ships as a single Rust binary (`skill-pool`). The companion
binary `skill-pool-capturer` (Phase 4 daemon) is documented in
[Phase 4 — Capture](Phase-4-Capture.md).

## Global flags

Every subcommand accepts:

| Flag         | Env var               | Purpose                                                          |
|--------------|-----------------------|------------------------------------------------------------------|
| `--config`   | `SKILL_POOL_CONFIG`   | Path to config TOML (default: `~/.skill-pool/config.toml`).      |
| `--registry` | `SKILL_POOL_REGISTRY` | Override registry URL for this invocation.                       |

Common env vars across many commands:

- `SKILL_POOL_TENANT` — tenant slug for the active section.
- `SKILL_POOL_TOKEN` — bearer token if `config.toml` isn't authoritative.
- `RUST_LOG` / `SKILL_POOL_LOG` — tracing filter (default `warn,skill_pool=info`).
- `SKILL_POOL_NO_BANNER=1` — suppress the per-tenant startup banner.

> The `--config` flag is honored end-to-end but `config.toml`'s
> `[registry]` section takes precedence over flag parsing in a few
> code paths — see [FAQ](FAQ.md) for the gotcha. When in doubt, set
> `SKILL_POOL_REGISTRY` env.

---

## `init`

**Source:** `cli/src/cmd/init.rs:1`

Writes a starter `.skill-pool/manifest.toml` in the current directory.
The manifest carries the project's stack tags and the list of skills,
agents, and slash-commands the project needs.

```bash
skill-pool init
# wrote .skill-pool/manifest.toml
```

After `init`, edit the file or use `skill-pool add <slug>` to populate
it (`add` appends to the right array based on `--kind`).

---

## `login`

**Source:** `cli/src/cmd/login.rs:1`

Authenticate against a registry and persist the token to
`~/.skill-pool/config.toml`.

| Flag         | Required | Description                                  |
|--------------|----------|----------------------------------------------|
| `--registry` | yes      | Registry base URL (e.g. `https://acme.skill-pool.example.com`). |
| `--tenant`   | yes      | Tenant slug.                                  |

```bash
skill-pool login \
  --registry https://acme.skill-pool.example.com \
  --tenant acme
# Paste your token (won't echo): ********************
# saved to ~/.skill-pool/config.toml [acme]
```

The token is read from stdin and stored under a tenant-namespaced
section, so a developer who belongs to two tenants can hold two
sections in the same config file.

---

## `detect`

**Source:** `cli/src/cmd/detect.rs:1`

Fingerprint the current project's stack from filesystem signals
(`package.json`, `Cargo.toml`, `pyproject.toml`, `go.mod`, etc.).
Caches the result in `.skill-pool/detected.json` keyed by mtime.

| Flag         | Description                                         |
|--------------|-----------------------------------------------------|
| `--json`     | Emit JSON instead of a human-friendly summary.      |
| `--no-cache` | Ignore the cache and force a fresh scan.            |

```bash
skill-pool detect
# stack: rust, axum, postgres, tokio

skill-pool detect --json
# {"stack":["rust","axum","postgres","tokio"],"detected_at":"…"}
```

`detect` is also called transparently by `bootstrap` — you rarely run
it standalone.

---

## `bootstrap`

**Source:** `cli/src/cmd/bootstrap.rs:1`

The canonical "onboard a new project" command. Detects the stack,
asks the registry which skills it recommends (tag intersection
fallback to semantic similarity), and (with confirmation) adds them
to the manifest and installs.

| Flag        | Short | Description                                                        |
|-------------|-------|--------------------------------------------------------------------|
| `--yes`     | `-y`  | Skip the Y/n confirmation prompt.                                  |
| `--detect`  |       | Re-run detection even if the manifest already has a stack.          |
| `--dry-run` |       | Show what would be added without writing or calling `ensure`.       |

```bash
cd ~/projects/my-rust-app
skill-pool bootstrap
# detected: rust, axum, postgres
# recommended:
#   - rust-axum-handler@^1.2
#   - sqlx-migrations@1.0.0
#   - tenant-ctx@*
# proceed? [y/N] y
# ✓ 3 skills installed in .claude/skills/
```

The recommendation pass is in
`server/src/routes/skills.rs::get_recommended`.

---

## `ensure`

**Source:** `cli/src/cmd/ensure.rs:1`

Install everything in the project manifest into
`.claude/skills/`. Idempotent — re-running on a fully-installed
project is a no-op.

| Flag             | Description                                                   |
|------------------|---------------------------------------------------------------|
| `--quiet`        | Suppress per-skill progress lines. Used by the direnv hook.   |
| `--no-telemetry` | Skip the best-effort `view` event POST per installed skill.   |

Behaviour:

1. Load `.skill-pool/manifest.toml`.
2. For each `[[skills]]`, `[[agents]]`, `[[commands]]` entry, call
   `GET /v1/skills/{slug}/deps` (skills only) to walk the closure.
3. Dedupe by `(slug, kind)`, sort leaves-first.
4. For anything not in `~/.skill-pool/library/<tenant>/<slug>@<ver>/`,
   download + extract.
5. Symlink each library entry into `.claude/skills/`.
6. (If not `--no-telemetry`) POST one `view` event per installed
   skill.

```bash
skill-pool ensure
# ✓ rust-axum-handler@1.2.3
# ✓ sqlx-migrations@1.0.0
# ✓ tenant-ctx@2.1.0 (transitive)
```

Used by the Claude Code `SessionStart` hook (`--quiet`) — see
`hook-install`.

---

## `add`, `add-agent`, `add-command`

**Source:** `cli/src/cmd/add.rs:1`

Add an entry to the manifest and install it. Three variants for the
three `kind` discriminators.

| Verb         | Manifest array         | Catalog `kind` |
|--------------|------------------------|----------------|
| `add`        | `[[skills]]`           | `skill`        |
| `add-agent`  | `[[agents]]`           | `agent`        |
| `add-command`| `[[commands]]`         | `command`      |

```bash
skill-pool add rust-axum-handler
# ✓ rust-axum-handler@1.2.3 added to manifest and installed

skill-pool add-agent code-reviewer
# ✓ code-reviewer (agent) added

skill-pool add-command rebase
# ✓ rebase (command) added
```

Internally `add-agent` and `add-command` are convenience wrappers over
`add` with `--kind` plumbed through (`cli/src/cmd/add.rs:run_with_kind`).

---

## `search`

**Source:** `cli/src/cmd/search.rs:1`

Search the registry. With no query, lists all skills.

| Flag                | Description                                                    |
|---------------------|----------------------------------------------------------------|
| `[QUERY]` (positional) | Substring matched against slug + description (ILIKE).        |
| `--tags`            | Comma-separated tags; ALL must be present on a result.         |
| `--limit`           | Limit results (1..200, default 50).                            |
| `--json`            | Emit JSON instead of a table.                                  |
| `--semantic <TEXT>` | Rank by cosine similarity of `description_embedding`.          |
| `--min-similarity`  | Minimum cosine similarity (0.0..1.0) when `--semantic` is set. |

```bash
skill-pool search axum
# slug                  version  description                                    tags
# rust-axum-handler     1.2.3    Tenant-scoped axum extractor pattern.          rust,axum

skill-pool search --tags rust,postgres
skill-pool search --semantic "how to write a database migration"
skill-pool search --json | jq '.[].slug'
```

`--semantic` requires the server to be built with `--features
fastembed`. Without it, the server returns 400 (`semantic search is
not enabled on this server`).

---

## `publish`

**Source:** `cli/src/cmd/publish.rs:1`

Publish a local skill directory to the registry.

| Arg/Flag          | Description                                                         |
|-------------------|---------------------------------------------------------------------|
| `<DIR>` (positional) | Path to the skill directory.                                    |
| `--slug`          | Override the slug. Defaults to frontmatter `name`, then dir name.   |
| `--version`       | **Required.** Semver string (e.g. `1.0.0`).                         |
| `--kind`          | `skill` (default), `agent`, or `command`.                           |

```bash
skill-pool publish ./my-skill --version 1.0.0
skill-pool publish ./my-agent --version 0.2.0 --kind agent
skill-pool publish ./my-command --version 0.1.0 --kind command --slug rebase
```

Validation happens server-side: frontmatter parses, `description` ≤
1536 chars, no `/home/`-style absolute paths in the body, gitleaks
secret scan, SHA-256 stored alongside. See
[API Reference](API-Reference.md#post-v1skills).

---

## `capture`

**Source:** `cli/src/cmd/capture.rs:1`

Capture a local skill directory as a **draft**. Drafts land in the
curator inbox at `/drafts` in the web portal; a reviewer assigns a
version at publish time. See [Phase 4 — Capture](Phase-4-Capture.md).

| Arg/Flag         | Description                                                              |
|------------------|--------------------------------------------------------------------------|
| `<DIR>`          | Path to the candidate skill directory.                                   |
| `--slug`         | Override the slug.                                                       |
| `--notes`        | Free-form note for the reviewer ("why this matters").                    |
| `--tags`         | Extra tags (comma-separated). Merged with frontmatter tags.              |
| `--allow-secret` | Skip the secret-scan gate. Findings logged but capture proceeds.         |

```bash
mkdir lesson-axum
# ... write lesson-axum/SKILL.md ...
skill-pool capture ./lesson-axum \
  --notes "Found while fixing the SCIM list endpoint — PR #42" \
  --tags rust,axum
```

---

## `capture-score`

**Source:** `cli/src/cmd/capture_score.rs:1`

Score a Claude Code session for "this was worth capturing" signals.
Designed to run as the **Stop hook** — reads the hook payload from
stdin, runs deterministic rules (no LLM, no network), persists the
score under `~/.skill-pool/sessions/`.

Exits 0 on any error so the hook never blocks the user.

| Flag          | Description                                              |
|---------------|----------------------------------------------------------|
| `--from-file` | Read the hook payload from a file instead of stdin (for debugging). |

Signals scored (see [Phase 4 — Capture](Phase-4-Capture.md#signals-scored-today)):

| Rule | Weight | Trigger |
|---|---:|---|
| Explicit marker | 1000 | "remember this" / "TIL" / `/capture-skill` |
| Test recovery | 50 | failing → passing test recovery |
| Edit retries | 30 | >3 failed `Edit`/`Write` on same file |
| Cross-session recurrence | 30 | same fingerprint in 3+ distinct sessions |
| Novel command | 15 | failed Bash stem not in shell history (capped at 3) |
| Long session | 5 | >20 assistant turns |

Default draft-worthy threshold: **score ≥ 100**.

```bash
# Usually invoked by Claude Code, not by you:
echo '{"session_id":"abc",…}' | skill-pool capture-score

# For debugging:
skill-pool capture-score --from-file ./hook-payload.json
```

---

## `capture-queue`

**Source:** `cli/src/cmd/capture_queue.rs:1`

Phase 4 SessionEnd hook. Reads the score that `capture-score` wrote;
if the total exceeds threshold, drops a marker file under
`~/.skill-pool/queue/<session_id>.queued`. Exits 0 on any failure.

| Flag           | Env var                          | Description                                |
|----------------|----------------------------------|--------------------------------------------|
| `--session-id` | `CLAUDE_SESSION_ID`              | Session id to queue. Defaults to env.      |
| `--threshold`  | `SKILL_POOL_CAPTURE_THRESHOLD`   | Min score to queue (default **50**).       |

Threshold precedence: flag → env → default. The default of 50 is
deliberately lower than the per-turn `capture-score` draft threshold
of 100 — SessionEnd fires once per session, so we surface more
sessions to the LLM gate downstream.

```bash
# Invoked by Claude Code:
CLAUDE_SESSION_ID=… skill-pool capture-queue
```

---

## `capture-status`

**Source:** `cli/src/cmd/capture_status.rs:1`

List persisted session scores, ranked. The `★` marks draft-worthy
sessions (score ≥ 100).

| Flag     | Description                              |
|----------|------------------------------------------|
| `--json` | Dump the raw records for piping.         |

```bash
skill-pool capture-status
# 12 sessions scored (3 ≥ draft threshold of 100)
#
#   SCORE  TURNS  CWD                SESSION
#  ★1050   3      /proj/foo          signals-1
#  ★ 130  18      /proj/bar          a4b2c1d…
#     5   26      /proj/baz          f8e9d2c…
```

---

## `capture-run`

**Source:** `cli/src/cmd/capture_run.rs:1`

Phase 4.6 LLM capturer. Two-stage pipeline (Haiku extractor → Sonnet
drafter → POST `/v1/drafts`). Idempotent: a session whose
`capture_state` is set is skipped. Designed for a systemd user timer
(Mode A) or invoked by the long-lived daemon (Mode B).

| Flag            | Env var                          | Default | Description                                       |
|-----------------|----------------------------------|---------|---------------------------------------------------|
| `--limit`       |                                  | 5       | Max sessions per pass (cost cap).                 |
| `--dry-run`     |                                  | false   | Show what would be processed; no LLM calls.       |
| `--stage1-model`|                                  | `claude-haiku-4-5-…` | Override Stage 1 model.                |
| `--stage2-model`|                                  | `claude-sonnet-4-6`  | Override Stage 2 model.                |
| `--allow-secret`|                                  | false   | Skip the client-side secret scan.                 |
| `--no-notify`   | `SKILL_POOL_CAPTURE_NO_NOTIFY=1` | false   | Suppress per-draft desktop notification.          |

Required env:

- `ANTHROPIC_API_KEY` — for the Messages API.
- `SKILL_POOL_REGISTRY` (or saved config from `skill-pool login`).

```bash
skill-pool capture-run                # default: up to 5 sessions
skill-pool capture-run --limit 20
skill-pool capture-run --dry-run
skill-pool capture-run --stage1-model claude-haiku-4-5-20251001 \
                       --stage2-model claude-sonnet-4-6
```

Full pipeline detail in [Phase 4 — Capture](Phase-4-Capture.md#pipeline).

---

## `direnv-install`

**Source:** `cli/src/cmd/direnv_install.rs:1`

Install the direnv helper into `~/.config/direnv/lib/` so `.envrc`
files can use `use skill_pool`. The helper is embedded in the binary
at compile time — no network call.

| Flag      | Description                                                          |
|-----------|----------------------------------------------------------------------|
| `--force` | Overwrite if a different version is already present.                 |

```bash
skill-pool direnv-install
# wrote ~/.config/direnv/lib/use_skill_pool.sh
```

Then in any project:

```bash
echo 'use skill_pool' >> .envrc
direnv allow
# triggers `skill-pool ensure --quiet` on cd
```

---

## `hook-install`

**Source:** `cli/src/cmd/hook_install.rs:1`

Install Claude Code hooks into `.claude/settings.json`. Always
installs the `SessionStart` hook (`skill-pool ensure --quiet`); with
`--with-scorer`, also installs the `Stop` hook (`skill-pool
capture-score`) and the `SessionEnd` hook (`skill-pool
capture-queue`).

| Flag            | Description                                              |
|-----------------|----------------------------------------------------------|
| `--remove`      | Remove all skill-pool hooks instead of installing.       |
| `--print`       | Print the merged `settings.json` to stdout; don't write. |
| `--with-scorer` | Also install Stop + SessionEnd hooks (Phase 4).          |

The CLI preserves all other settings — install and remove operate on a
JSON merge, never an overwrite.

```bash
# Phase 3 — just the install hook:
skill-pool hook-install

# Phase 4 — install + scorer + queue:
skill-pool hook-install --with-scorer

# Inspect what would be written:
skill-pool hook-install --with-scorer --print

# Remove everything:
skill-pool hook-install --remove
```

---

## `doctor`

**Source:** `cli/src/cmd/doctor.rs:1`

Diagnose: list loaded skills, dangling symlinks, drift between the
manifest and the on-disk library, server reachability, version drift.

| Flag     | Description                              |
|----------|------------------------------------------|
| `--json` | Emit JSON instead of a human summary.    |

```bash
skill-pool doctor
# config:  ~/.skill-pool/config.toml [acme]
# server:  https://acme.skill-pool.example.com  ✓ reachable
# library: ~/.skill-pool/library/acme/  (8 skills, 0 dangling)
# project: ~/projects/my-app
#   manifest: 3 skills, 1 agent
#   .claude/skills: 4 symlinks, all healthy
#   ✓ no drift
```

What it checks:

1. Config file exists and parses.
2. The configured registry returns 200 on `/v1/healthz`.
3. Every entry in `.skill-pool/manifest.toml` has a matching
   library entry (or warns about a future install).
4. Every symlink under `.claude/skills/` points at a real library
   entry (or warns about dangling links).
5. Decay status of installed skills (skills marked
   `archive_candidate` get a yellow warning).

---

## Configuration file format

`~/.skill-pool/config.toml`:

```toml
# A "web_url" outside any section is read by the capturer for notification links.
web_url = "https://acme.skill-pool.example.com"

[registry]
url    = "https://acme.skill-pool.example.com"
tenant = "acme"
# token is stored separately; this section is just the default tenant.

# Multi-tenant developers stack sections:
[tenant.acme]
url   = "https://acme.skill-pool.example.com"
token = "spk_…"

[tenant.globex]
url   = "https://globex.skill-pool.example.com"
token = "spk_…"
```

The `[registry]` section is for backward compatibility — newer
versions prefer `[tenant.*]` sections.

## Project manifest format

`.skill-pool/manifest.toml` (relative to the project root):

```toml
[project]
stack = ["rust", "axum", "postgres"]

[[skills]]
slug    = "rust-axum-handler"
version = "^1.2"

[[skills]]
slug    = "sqlx-migrations"
version = "1.0.0"

[[agents]]
slug = "code-reviewer"

[[commands]]
slug = "rebase"
```

Full schema reference: `docs/manifest-schema.md` in the repo.

## Where to read next

- [Phase 4 — Capture](Phase-4-Capture.md) — `capture-*` subcommand deep dive
- [Phase 5 — Lifecycle](Phase-5-Lifecycle.md) — what happens after publish
- [API Reference](API-Reference.md) — what each CLI command POSTs
- [FAQ](FAQ.md) — `--config` flag gotcha, host-vs-container port, etc.

## Cross-links into the codebase

- `cli/src/main.rs` — the clap enum (source of truth for flags)
- `cli/src/cmd/` — per-command implementations
- `cli/src/config.rs` — config file loader
- `cli/src/lib.rs` — client wrappers used by every subcommand
- `cli/src/scorer.rs` — deterministic Phase 4.5 scorer
- `cli/src/capturer.rs` — Phase 4.6 LLM pipeline
