# skill-pool

Self-hosted registry, CLI, and web UI for sharing Claude Code skills, subagents, and slash commands across a developer team.

**Status:** Phase 0 — spike. See [plan](https://_local_/plan).

## What it solves

Claude Code's extensibility (skills, subagents, commands, plugins, hooks, MCP) is powerful, but distribution and lifecycle across a team are unsolved. `skill-pool` is the team layer:

1. **Catalog** — one source of truth for the team's skills/agents/commands, with a CLI and web UI.
2. **Auto-bootstrap** — when a developer enters a project, the right skills install themselves (with one-keystroke confirmation).
3. **Retrospective capture** — after a non-trivial fix, the system drafts a reusable skill for human review.

Read the plan at `~/.claude/plans/fluttering-swinging-lobster.md`.

## Phase 0 — validate the install path

Goal: prove `SKILL.md` files placed under `~/.claude/skills/` (or a project's `.claude/skills/`) are picked up by Claude Code.

```bash
nix develop                            # enter dev shell
scripts/install.sh --help              # install script usage
scripts/install.sh --library ./skills --target ~/.claude/skills test-skill
claude                                 # start Claude Code
# in session: /skills    → confirm `test-skill` is listed
# in session: ask "what is the skill pool test skill" → Claude should invoke it
```

## Layout

```
skill-pool/
├── flake.nix          # Nix dev shell + (later) packages and NixOS module
├── scripts/install.sh # Phase 0 symlink-based installer
├── skills/            # Sample skills (Phase 0 fixtures)
├── cli/               # Rust CLI (Phase 1)
├── server/            # Rust HTTP server (Phase 1)
├── web/               # Web UI (Phase 2)
└── docs/              # Schemas, API, verification procedures
```

## Phases

- **Phase 0** — spike: shell installer, one test skill.
- **Phase 1** — server + CLI MVP (skills only). ← *in progress*
- **Phase 2** — web UI for browse/edit/publish.
- **Phase 3** — auto-bootstrap (stack detection + direnv).
- **Phase 4** — retrospective capture (hooks + drafts inbox).
- **Phase 5** — lifecycle: embeddings, decay, dependencies, agents+commands.

## Testing

```bash
cargo test --workspace               # unit + integration (requires Docker)
cargo test --workspace --bins        # unit only (no Docker needed)
scripts/integration-test.sh          # human-driven smoke against docker-compose stack
```

The integration test (`server/tests/integration.rs`) brings up Postgres via
testcontainers, spawns the router on a random port, and asserts the full
publish → list → fetch flow plus tenant isolation (tenant B cannot see
tenant A's skills, even with a valid token).
