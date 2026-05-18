# Publish your first skill

A walkthrough — from "I have a useful pattern" to "the team can `skill-pool add` it." About five minutes start to finish.

## What you need

- The `skill-pool` CLI on `PATH`
- A registry token (mint one in the portal under Settings → Members, or via `skill-pool-server admin token-create`)
- A directory with one Markdown file you're proud of

## 1. Set up

Authenticate against your team's registry once per machine:

```bash
skill-pool login \
  --registry https://acme.skill-pool.example.com \
  --tenant   acme
# (you'll be prompted to paste the token; it's stored under
#  ~/.config/skill-pool/config.toml with 0600 permissions)
```

## 2. Make the skill directory

```bash
mkdir axum-handler-tip
cd axum-handler-tip
```

Create `SKILL.md`. The frontmatter is YAML; the body is plain Markdown.

```markdown
---
name: axum-handler-tip
description: Pattern for axum tenant-scoped extractors that avoids the
  borrow-checker dance with a request-scoped clone.
when_to_use: When building axum handlers that need TenantCtx + AppState.
tags: [rust, axum, tenant]
---

# axum-handler-tip

Implement `FromRequestParts` once on a wrapper type. Have it read the
`Host` header and clone the relevant pieces of `AppState` into the
extractor itself, so handlers stay borrow-clean…

(steps, code blocks, screenshots if you like)
```

## 3. Publish

```bash
skill-pool publish ./ --version 1.0.0
```

The CLI bundles the directory, ships it to the registry, and prints the
final slug + version. The server runs three checks before accepting:

- YAML frontmatter parses and has a non-empty `description`
- No absolute paths like `/home/<you>/…` (they identify the author and
  break on other machines)
- No obvious secrets (AWS keys, GitHub PATs, PEM blocks)

If a check fails, the CLI surfaces the message and the bundle isn't
stored — fix and re-run.

## 4. Verify

In the portal: open **Catalog**, the new card should be there. Or via CLI:

```bash
skill-pool search axum-handler
```

## 5. Anyone on the team can now install it

Their `.skill-pool/manifest.toml` gains:

```toml
[[skills]]
slug = "axum-handler-tip"
version = "*"
scope = "project"
```

Their `skill-pool ensure` (or direnv hook, or Claude SessionStart hook)
picks it up next time.

## Tips

- **Declare dependencies.** If your skill builds on another team skill,
  add `requires: [other-slug]` to the frontmatter. `skill-pool ensure`
  walks the transitive closure.
- **Use tags well.** They drive the catalog filter and the auto-bootstrap
  recommendations. `[rust, axum]` is more useful than `[backend]`.
- **The description is the abstract.** It's what shows up in search results
  and in the semantic-search ranking. Two clear sentences beat a stub.
- **Versions are semver.** Re-publishing `1.0.0` is a 400; bump to `1.0.1`.
