# skill-pool

> A multi-tenant catalog + per-developer CLI for Claude Code skills.
> Built in Rust + SvelteKit, deployable to NixOS, Kubernetes (Helm),
> or a single VPS with systemd + Caddy.

**skill-pool** turns "I figured out the axum extractor trick" into "the
whole team has it installed by tomorrow morning". A developer captures
a skill (or the LLM captures it for them from a session transcript), a
reviewer publishes it, every other developer's next `claude-code`
session loads it via the install side of the CLI.

The catalog is multi-tenant from row 1: every business query filters by
`tenant_id`, every object-storage key is namespaced under `{tenant_id}/`,
and a static-analysis test harness asserts the invariant at build time
(see [Multi-Tenancy](Multi-Tenancy.md)).

---

## 30-second pitch

1. Developer runs `skill-pool capture ./my-pattern` — or just installs
   the [Phase 4](Phase-4-Capture.md) hooks and lets the scorer + LLM
   capturer notice draft-worthy sessions for them.
2. A reviewer hits **Publish** in the web UI inbox (see the screenshots
   in [README](https://github.com/olafkfreund/skill_pool#screenshots)).
3. Every other developer's next session loads the new skill — the
   `SessionStart` hook (`skill-pool ensure --quiet`) reconciles
   `.claude/skills/` to the team manifest in under a second on the
   happy path.
4. Lifecycle keeps the catalog honest: usage events bump `use_count`,
   the decay sweep flags unused skills as `archive_candidate`, an
   admin button moves them to the graveyard (see [Phase-5-Lifecycle](Phase-5-Lifecycle.md)).

The same binary runs the registry, the CLI, the curator, and the
capturer daemon — `cargo build` produces three executables.

---

## Project status

Five phases shipped, manual quality gate #2 open:

| Phase | Theme | Status |
|---|---|---|
| **Phase 0** | Project scaffolding, repo skeleton, CI baseline | shipped |
| **Phase 1** | Multi-tenant server, Postgres + opendal, audit log, REST API | shipped |
| **Phase 2** | SSO (OIDC + SAML), SCIM provisioning, web portal v1 | shipped |
| **Phase 3** | CLI (`init`/`add`/`ensure`/`bootstrap`/`detect`), direnv, doctor | shipped |
| **Phase 4** | Retrospective capture: scorer, queue, two-stage LLM capturer, daemon | shipped |
| **Phase 5** | Lifecycle: decay, dependencies, MCP, agents+commands, git mirror | shipped |
| **Gate #2** | Manual QA pass on themed UI, capture cycle, SSO flows | **open** |

The "open" gate is the human-in-the-loop verification that a fresh
operator can stand up an instance, onboard a tenant, publish a skill,
capture one via the daemon, and roll the tenant theme — end-to-end —
without reading the source. See [Tenant-Onboarding](Tenant-Onboarding.md)
and [FAQ](FAQ.md) for what tends to bite you.

---

## Navigate

### Getting started

- [Tenant Onboarding](Tenant-Onboarding.md) — first-tenant playbook
- [CLI Reference](CLI-Reference.md) — every `skill-pool` subcommand
- [Bundled Skills](Bundled-Skills.md) — what ships in the catalog out of the box
- [FAQ](FAQ.md) — real failure modes from the first install

### Operator

- [Operator Guide](Operator-Guide.md) — every deploy path collated
- [SSO Setup](SSO-Setup.md) — OIDC and SAML, per-IdP notes
- [Custom Domain + ACME](Custom-Domain-ACME.md) — per-tenant hostnames
- [Theming](Theming.md) — palette, logo, font, custom CSS overlay

### Developer

- [Phase 4 — Capture](Phase-4-Capture.md) — scorer, queue, capturer daemon
- [Phase 5 — Lifecycle](Phase-5-Lifecycle.md) — decay, deps, agents, MCP, git mirror
- [MCP Integration](MCP-Integration.md) — Claude Code as a catalog client

### Reference

- [Architecture](Architecture.md) — component diagram + data flow
- [Multi-Tenancy](Multi-Tenancy.md) — shared vs dedicated, tenant resolution
- [API Reference](API-Reference.md) — every endpoint, grouped by tag
- [Decisions Log](Decisions-Log.md) — why each major design choice was made

### Other

- [README](https://github.com/olafkfreund/skill_pool#readme) — visual tour with screenshots and a GIF demo
- [Source tree](https://github.com/olafkfreund/skill_pool/tree/main)
- [Issue tracker](https://github.com/olafkfreund/skill_pool/issues)

---

## Useful entry points

- Want to read the code? `cli/src/main.rs` (CLI surface),
  `server/src/main.rs` (axum routes + state), `web/src/routes/`
  (SvelteKit portal).
- Want to see the wire format? [API Reference](API-Reference.md) is the
  one stop; the OpenAPI is generated from the same routes.
- Want to deploy? Start with [Operator Guide](Operator-Guide.md); pick a
  path; come back for [SSO Setup](SSO-Setup.md) once the server is up.

> Wiki is the canonical operator/developer documentation. The repo's
> `docs/` directory is the source these pages mirror — when in doubt,
> the code in `server/src/` wins.
