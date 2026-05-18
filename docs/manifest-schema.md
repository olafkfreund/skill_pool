# Project manifest

`<project>/.skill-pool/manifest.toml` — committed to the project repo. Describes which skills/agents/commands belong with this project. The CLI uses it for `ensure`, `add`, and (Phase 3) auto-bootstrap.

## Minimal example

```toml
[project]
stack = ["rust", "axum", "postgres", "nixos"]

[[skills]]
slug = "rust-axum-handler"
version = "^1.2"
scope = "project"

[[skills]]
slug = "sqlx-migrations"
version = "*"
scope = "project"

[[agents]]
slug = "sqlx-migration-reviewer"
```

## Fields

### `[project]`

| Field | Required | Description |
|---|---|---|
| `stack` | no | Tags describing the project's stack. Used for Phase 3 auto-bootstrap matching. |
| `tenant` | no | Pin a tenant for this project (overrides CLI config). Rare. |

### `[[skills]]`, `[[agents]]`, `[[commands]]`

| Field | Required | Default | Description |
|---|---|---|---|
| `slug` | yes | — | Registry slug. |
| `version` | no | `"*"` | Semver range. `"*"` = latest published. |
| `scope` | no | `"project"` | `"project"` = symlink into `./.claude/skills/`; `"personal"` = `~/.claude/skills/`. |

## Lifecycle

- **Created** by `skill-pool init`.
- **Mutated** by `skill-pool add <slug>` and `skill-pool remove <slug>`.
- **Consumed** by `skill-pool ensure` (the direnv / SessionStart hook).
- **Inspected** by `skill-pool doctor`.

## Idempotency

Every operation on the manifest is idempotent. Running `ensure` twice is a no-op the second time; the install script's symlink semantics (see `scripts/install.sh`) carry that property through to the filesystem.

## Why TOML

- Trivial human edits (PRs that add one skill should be a one-line diff)
- Parses fast in Rust
- No JSON quoting noise
