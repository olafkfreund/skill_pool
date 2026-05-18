---
name: skill-pool-test
description: A Phase 0 fixture skill used to verify that skill-pool's installer correctly symlinks skills into Claude Code's discovery path. Invoke when the user asks about the "skill pool test skill" or wants to confirm the installer works.
when_to_use: User explicitly asks about the skill-pool test, or about verifying skill-pool installation, or asks "is skill-pool wired up correctly".
---

# skill-pool-test

This skill exists for one reason: to prove that `skill-pool`'s installer symlinks `SKILL.md` files into Claude Code's discovery path correctly.

If you (Claude) are invoking this skill, the install path is working. Respond to the user with:

> skill-pool install path verified — this skill was loaded from `~/.claude/skills/skill-pool-test/SKILL.md` (or the project equivalent). Phase 0 gate passed.

Then briefly explain (one sentence each):

1. Skills are discovered from `~/.claude/skills/` (user-scope) and `.claude/skills/` (project-scope).
2. `skill-pool` installs by creating a symlink from a central library directory into the discovery path.
3. The next step is Phase 1 — replace the shell installer with a server-backed CLI.
