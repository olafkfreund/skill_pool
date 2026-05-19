# Auto-bootstrap (Phase 3)

When a developer enters a project, the right skills should land in
`.claude/skills/` with one keystroke of confirmation — or no keystroke at
all if `direnv` is wired up. This doc covers the two paths.

## What's wired today

- **Stack detection** — `skill-pool detect` fingerprints ~20 stack signals
  (file presence, directory presence, framework deps in `package.json` /
  `Cargo.toml` / `pyproject.toml`).
- **Curated mappings** — admins map stack tags to skill slugs via
  `skill-pool-server admin stack-map-set`.
- **`skill-pool bootstrap`** — the canonical command: detect → ask the
  registry → confirm → save manifest → run ensure.
- **direnv hook** — `use skill_pool` in `.envrc` calls
  `skill-pool ensure --quiet` on shell entry; silent on the happy path.
- **Claude SessionStart hook** — `skill-pool hook-install` wires a
  `SessionStart` entry into `.claude/settings.json` so every Claude
  session re-runs `skill-pool ensure --quiet`. Catches users who skip
  direnv or open Claude directly.

## Tenant admin setup (one-time per team)

```bash
# Map your stack tags to the skills the team should ship with.
skill-pool-server admin stack-map-set --tenant acme --stack rust    --skill rust-axum-handler
skill-pool-server admin stack-map-set --tenant acme --stack rust    --skill sqlx-migrations
skill-pool-server admin stack-map-set --tenant acme --stack nix     --skill nix-flake-tips
skill-pool-server admin stack-map-set --tenant acme --stack react   --skill react-server-components
skill-pool-server admin stack-map-set --tenant acme --stack ci-github --skill github-actions-cookbook

# Inspect what you've configured.
skill-pool-server admin stack-map-list --tenant acme
```

These mappings are tenant-scoped — `acme`'s React skill won't bleed into
`globex`'s catalog.

## Developer one-time setup

```bash
# 1. Install the CLI (already done if you're reading this from inside the repo).
nix-shell -p skill-pool

# 2. Authenticate against your team's registry.
skill-pool login --registry https://acme.skill-pool.example.com --tenant acme

# 3. Install the direnv hook.
skill-pool direnv-install
```

`direnv-install` copies a tiny shell library to
`~/.config/direnv/lib/use_skill_pool.sh` (or `$XDG_CONFIG_HOME/direnv/lib/`
if set). The library is **embedded in the CLI binary** — no network
fetch.

## Per-project — two patterns

### Pattern A: explicit bootstrap (no direnv)

```bash
cd my-new-project
skill-pool bootstrap
# stack: rust, axum, nix
# Recommended skills for this project (4):
#   + rust-axum-handler
#   + sqlx-migrations
#   + nix-flake-tips
#   + github-actions-cookbook
# Add these to the manifest and install? [Y/n]
```

Flags:
- `--yes` — skip the prompt (use this from non-interactive shells)
- `--detect` — re-run detection even if the manifest already has a stack
- `--dry-run` — show the plan without saving the manifest or installing

### Pattern B: direnv on autopilot

In the project's `.envrc`:

```
use skill_pool              # silent ensure on every `cd` into the dir
```

Or for a fresh project that hasn't been bootstrapped yet:

```
use skill_pool bootstrap    # detect + recommend + install on the first cd
```

Then:

```bash
direnv allow                # blesses the .envrc
```

Subsequent `cd` into the project runs `skill-pool ensure --quiet` —
silent on the happy path, only prints when something changed or failed.

The hook **never blocks shell entry**. If the registry is unreachable or
the CLI isn't installed, you get a `log_status` warning and the shell
loads normally.

### Pattern C: Claude SessionStart hook

Complements direnv — direnv fires on shell entry, the SessionStart hook
fires when Claude opens a session in the project. Useful for users who
skip direnv or launch Claude directly without `cd`-ing first.

```bash
cd my-project
skill-pool hook-install
# installed skill-pool SessionStart hook in /path/.claude/settings.json
```

What it writes into `.claude/settings.json`:

```json
{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "*",
        "hooks": [
          { "type": "command", "command": "skill-pool ensure --quiet", "timeout": 30 }
        ]
      }
    ]
  }
}
```

Existing keys (`model`, `permissions`, other `SessionStart` entries) are
preserved — the install merges JSON, it doesn't overwrite.

Flags:
- `--remove` — strip our hooks (leaves the rest of the file intact)
- `--print` — dump the merged settings to stdout without writing
- `--with-scorer` — also install the Phase 4.5 `Stop` hook
  (`skill-pool capture-score`). See `docs/capture.md`.

Like direnv, the hooks are best-effort: if `skill-pool` isn't on PATH or
the registry is unreachable, the session continues normally (Claude's
hook system swallows non-fatal failures).

## Manifest reference

`.skill-pool/manifest.toml` lives in the repo, gets committed.

```toml
[project]
stack = ["rust", "axum", "nix"]      # seeded by `skill-pool init` from detection

[[skills]]
slug = "rust-axum-handler"
version = "*"                         # latest; or "^1.2" for semver-pin
scope = "project"                     # "project" → ./.claude/skills/
                                      # "personal" → ~/.claude/skills/

[[skills]]
slug = "sqlx-migrations"
version = "*"
scope = "project"
```

The manifest is what `skill-pool ensure` reads. Both `bootstrap` and
`add` append to it.

## Matching tiers

`GET /v1/bootstrap?stack=tag1,tag2,…` unions three tiers and caps the
response at eight slugs:

1. **Curated** — `tenant_stack_mappings` rows the admin maintains.
   Highest precision; this is the team's intentional shape.
2. **Tag intersection** — published skills whose `tags` array overlaps
   the stack tags. Ranks by overlap-count DESC, then `created_at` DESC
   so freshest broad-matches surface first.
3. **Semantic similarity** — embeds the joined stack string (e.g.
   `"rust axum postgres"`) and ranks published skills by cosine
   distance over their description embeddings. Skipped entirely when no
   embedder is configured (`NullEmbedder` / default build); the catalog
   degrades gracefully, no 5xx.

Dedup priority is `curated > tagged > semantic`: a slug surfaced by a
higher tier is removed from lower tiers before the union. Pass
`?debug=1` to see per-tier attribution under `tier_breakdown` (omitted
otherwise so the default response stays minimal).

## What's NOT yet wired (later iterations)

- **Manifest deep-parse tier** — currently `detect` reads top-level
  `Cargo.toml`. Workspace member traversal + AST parsing for richer
  framework distinctions lands later.
- **LLM fallback tier** — deferred indefinitely. The master plan
  reserved a fourth tier for a one-shot Haiku call when fingerprints
  yield no useful tags, but in practice the three tiers above cover
  the long tail at zero token-cost and without requiring an Anthropic
  API key on the server. We may revisit if real telemetry shows
  measurable zero-result requests we can't already cover.
- **Web UI for stack mappings** — admins set them via CLI today.
