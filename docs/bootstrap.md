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

## What's NOT yet wired (later iterations)

- **SessionStart Claude Code hook** — re-runs `ensure --quiet` when a
  Claude session starts in this dir. Currently you need `direnv` to get
  per-`cd` updates; once the SessionStart hook lands, even plain shells
  pick up new skills as soon as Claude opens.
- **Manifest deep-parse tier** — currently `detect` reads top-level
  `Cargo.toml`. Workspace member traversal + AST parsing for richer
  framework distinctions lands later.
- **LLM fallback tier** — when fingerprints yield no useful tags
  (a brand-new repo with just `README.md`), fall back to a one-shot
  Haiku call. Off by default.
- **Tag intersection + embedding matching** — Phase 5; broadens
  recommendations beyond the curated map.
- **Web UI for stack mappings** — admins set them via CLI today.
