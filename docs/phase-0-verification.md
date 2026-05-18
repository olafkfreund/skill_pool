# Phase 0 — verification

**Gate:** Claude Code discovers and invokes a skill that `skill-pool` installed.

## Prerequisites

- `claude` CLI installed and working
- This repo cloned somewhere local
- Either Nix (`nix develop`) or just plain Bash

## Procedure

### 1. Lint the installer

```bash
cd /path/to/skill-pool
shellcheck scripts/install.sh
shfmt -i 2 -ci -d scripts/install.sh
```

Both should exit 0. (`-i 2 -ci` keeps shfmt aligned with the project's 2-space indent style; without flags shfmt defaults to tabs.)

### 2. Inspect available skills (no install)

```bash
scripts/install.sh --library ./skills --list
```

Expect output containing `skill-pool-test` and its description.

### 3. Dry-run install

```bash
scripts/install.sh --library ./skills --dry-run test-skill
```

Expect to see `link: test-skill (... -> ...)` and `(dry-run; no changes made)`.

### 4. Real install (user scope)

```bash
scripts/install.sh --library ./skills test-skill
ls -la ~/.claude/skills/test-skill
readlink ~/.claude/skills/test-skill
```

`~/.claude/skills/test-skill` should be a symlink pointing into this repo's `skills/test-skill`.

### 5. Idempotency check

```bash
scripts/install.sh --library ./skills test-skill
```

Expect `ok    (already linked): test-skill`. No errors.

### 6. End-to-end Claude check

```bash
cd ~                      # ensure we are *outside* the repo, using user scope
claude
```

Inside the Claude Code session:

```
/skills
```

`skill-pool-test` should be listed. Then ask:

> Is skill-pool wired up correctly?

Claude should invoke the `skill-pool-test` skill and confirm the install path is working (the skill instructs it to reply with a specific message).

### 7. Uninstall

```bash
scripts/install.sh --library ./skills --uninstall test-skill
ls ~/.claude/skills/test-skill 2>/dev/null || echo "removed"
```

Expect `removed`.

## Pass / fail

- **Pass:** Steps 1-7 all succeed and Claude invokes the skill in step 6. Phase 0 gate cleared; proceed to Phase 1.
- **Fail in steps 1-5:** Installer bug. Fix and re-run.
- **Fail in step 6:** Claude Code is not discovering `~/.claude/skills/`. Check `claude --version`, confirm the skills feature is enabled for your account tier, and inspect `~/.claude/settings.json` for `skillOverrides`. The plan's `claude-code-guide` research notes that user-scope skills under `~/.claude/skills/<slug>/SKILL.md` are discovered automatically — no settings change should be needed.
- **Fail in step 7:** Uninstall bug. Fix and re-run.

## What this gate proves

If green, we know:

1. The SKILL.md format and frontmatter we plan to use are valid.
2. Symlink-based install (vs copy) is acceptable to Claude Code's discoverer.
3. The user-scope discovery path (`~/.claude/skills/`) works as documented.

That's enough to commit to the Phase 1 design (server + CLI) without re-architecting later.
