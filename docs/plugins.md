# Plugins

> Per-tenant Claude Code plugin marketplace built on the existing skills/agents/commands registry.

A **plugin** is Claude Code's packaging primitive that bundles one or more skills, agents, commands, hooks, MCP servers, LSP servers, and monitors into one installable unit ([Plugins reference](https://code.claude.com/docs/en/plugins-reference)). A **marketplace** is the catalog Claude Code consumes via `/plugin marketplace add <url>`; each marketplace lists plugins and where to fetch them ([Plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces)).

skill-pool already distributes skills, agents, and commands as atomic units. Plugins are the next layer above that: a curator picks N already-registered atomic pieces, packages them into one plugin, and exposes the result through their tenant's marketplace. A user then runs two commands in Claude Code and the bundle is installed.

## Plugins vs skills, agents, commands

| Primitive | What it is | How it ships in Claude Code |
|---|---|---|
| Skill | A `SKILL.md` (plus optional supporting files) Claude can invoke by name | Either user-installed under `~/.claude/skills/` or bundled inside a plugin |
| Agent | A markdown file declaring a specialized subagent | Either user-installed under `~/.claude/agents/` or bundled inside a plugin |
| Command | A flat markdown file Claude treats as a slash command | Either user-installed under `~/.claude/commands/` or bundled inside a plugin |
| Plugin | A directory bundling any of the above + hooks + MCP/LSP servers + monitors + themes + settings, with a `.claude-plugin/plugin.json` manifest | Installed via `/plugin install <name>@<marketplace>`; Claude Code clones the plugin's source into `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/` |

A plugin is therefore not a *replacement* for the registry's existing primitives — it is a *composition* of them. Every skill or agent inside a skill-pool-authored plugin remains an addressable, versioned entry in this tenant's skill/agent/command catalogue. Publishing a plugin does not duplicate that content; it points at it.

## How a plugin gets into a Claude Code session

The end-to-end flow, with skill-pool standing in as the marketplace:

1. The curator assembles a plugin in the skill-pool portal (or via the CLI), picking which registered skills/agents/commands to bundle and adding inline hooks/MCP/monitor blobs if the plugin needs them.
2. The plugin lands in the tenant's `plugin_marketplace_entries` table. The tenant's marketplace endpoint reflects the new entry on next fetch — see [Per-tenant marketplace](#per-tenant-marketplace) below.
3. A developer runs `/plugin marketplace add https://<tenant>.<skill-pool-host>` inside Claude Code. The Claude Code client fetches `.claude-plugin/marketplace.json` from that host, validates it, and registers the marketplace under the user's chosen scope (see [Plugin installation scopes](https://code.claude.com/docs/en/plugins-reference#plugin-installation-scopes)).
4. The developer runs `/plugin install <plugin-name>@<marketplace-name>`. Claude Code reads the plugin entry's `source` field from `marketplace.json` and clones or copies the plugin into the local cache at `~/.claude/plugins/cache/<marketplace>/<plugin>/<version>/` ([Plugin caching and file resolution](https://code.claude.com/docs/en/plugins-reference#plugin-caching-and-file-resolution)).
5. The plugin's components (skills, agents, hooks, MCP servers, monitors, themes) light up in the next session.

Claude Code's local cache is the source of truth at runtime — the marketplace endpoint and the plugin's git source are not contacted again until `/plugin marketplace update` or `/plugin update` runs. skill-pool's job ends at "serve the right bytes at the right URLs."

## Three sourcing modes

skill-pool supports three ways a plugin can enter the tenant's marketplace. The plugin's `sourcing_mode` is persisted on the row and surfaced everywhere the plugin appears.

### Internal

The plugin is authored inside skill-pool. The portal's composer or the CLI assembles a manifest from skills/agents/commands already registered in the tenant, plus any inline hooks/MCP/monitor blobs. skill-pool materialises the plugin tree on the server side and hosts it from its own git endpoint, so the marketplace entry's `source` points at `https://<tenant>.<skill-pool-host>/git/plugins/<slug>.git`. Versions correspond to git refs in that repo. This is the path that reuses the most of the existing registry: every bundled skill is a slug in `skills`, every agent is a slug in `agents`.

### External

The plugin lives in someone else's git repo (a public GitHub repo, an internal GitLab project, etc.) and skill-pool just lists it. The marketplace entry's `source` is whatever git URL the curator pasted in — see the [`github`](https://code.claude.com/docs/en/plugin-marketplaces#github-repositories), [`url`](https://code.claude.com/docs/en/plugin-marketplaces#git-repositories), and [`git-subdir`](https://code.claude.com/docs/en/plugin-marketplaces#git-subdirectories) source types in the Claude Code spec. Claude Code clones the plugin directly from that upstream; skill-pool never holds the bytes. Cheapest mode to operate, but it requires every developer's Claude Code client to be able to reach the upstream URL.

### Mirror

The plugin lives upstream but skill-pool clones it into local storage, refreshes it on a schedule, and serves the mirror from its own git endpoint. The marketplace entry's `source` points at skill-pool, not the upstream. This is the path for air-gapped tenants, slow upstream hosts, or curators who want a snapshot they control. The mirror worker is the same async pattern as the Phase 4 capture pipeline — see [Background mirror refresh](#background-mirror-refresh) below.

| Mode | Curator effort | Egress required from developers' Claude Code? | skill-pool stores the bytes? |
|---|---|---|---|
| Internal | High (assemble in portal) | No (skill-pool only) | Yes |
| External | Low (paste URL) | Yes (upstream URL) | No |
| Mirror | Medium (paste URL, configure refresh) | No (skill-pool only) | Yes |

## Per-tenant marketplace

skill-pool exposes one marketplace per tenant. Every tenant gets:

- `GET https://<tenant>.<skill-pool-host>/.claude-plugin/marketplace.json` — the Claude Code-spec marketplace catalogue for this tenant. Public read; rate-limited.
- `GET https://<tenant>.<skill-pool-host>/git/plugins/<slug>.git/...` — the dumb-HTTP git endpoint for internal and mirrored plugins. Public read; rate-limited.

This alignment is deliberate: it mirrors how Projects and Plans are scoped, and it gives every tenant an independent curation surface without cross-tenant leakage. The same `tenant_id` invariant that gates the rest of the schema (see `docs/tenancy.md`) gates plugin reads and writes.

### Why per-tenant

- **Multi-tenancy alignment.** Every other curated artefact in skill-pool — skills, agents, commands, projects, plans — is per-tenant. Plugins inherit the same model rather than introducing a global namespace.
- **Curation.** Each tenant decides what its developers see. The `acme` tenant's marketplace is a different list from the `globex` tenant's, even if they share some upstream sources.
- **Theming and branding.** The public marketplace browser at `/marketplace` (see [Issue #7](https://github.com/olafkfreund/skill_pool/issues/37) in the plugins epic) renders under the tenant's existing portal branding.
- **Trust boundary.** Claude Code's trust model is per-marketplace: the user explicitly opts in by running `/plugin marketplace add <url>`. Per-tenant URLs map that opt-in to skill-pool's existing tenancy boundary.

### Background mirror refresh

Mirrored plugins have a `pull_interval_secs` configured per plugin (default 24h). A background worker wakes every 60s, finds plugins whose `last_pulled_at + pull_interval_secs` is in the past, pulls up to N in parallel (default 4), and updates the local git endpoint. Pull failures keep the last-good copy serving and surface a warning chip on the plugin's detail page in the portal. The pattern matches the plan auto-refresh sweep already in production (`docs/plans.md`, search "Auto-refresh").

## Architecture diagram

```
                              ┌─────────────────────────────────────┐
                              │     skill-pool-server (Rust)        │
                              │                                     │
                              │  ┌──────────────────────────────┐   │
   Claude Code client         │  │ /v1/plugins (CRUD, auth req'd)│  │
   ┌────────────────────┐     │  └──────────────────────────────┘   │
   │ /plugin marketplace│     │  ┌──────────────────────────────┐   │
   │     add <url>      ├─────┼─►│ /.claude-plugin/             │   │
   │                    │     │  │     marketplace.json          │   │
   │ /plugin install    │     │  │ (per-tenant, public read)     │   │
   │     <name>@<mp>    │     │  └──────────────┬───────────────┘   │
   └────────┬───────────┘     │                 │                   │
            │                 │                 │ assembled from    │
            │ git-clone       │                 ▼                   │
            │ plugin source   │  ┌──────────────────────────────┐   │
            │                 │  │ plugin_marketplace_entries   │   │
            │      ┌──────────┼─►│ (Postgres, tenant-scoped)    │   │
            │      │          │  └──────────────┬───────────────┘   │
            │      │          │                 │                   │
            │      │          │                 ▼                   │
            │      │          │  ┌──────────────────────────────┐   │
            │      │          │  │ plugins + plugin_contents    │   │
            │      │          │  │ (FK → skills/agents/commands)│   │
            │      │          │  └──────────────────────────────┘   │
            │      │          │                                     │
            │      │          │  ┌──────────────────────────────┐   │
            │      └──────────┼─►│ /git/plugins/<slug>.git/...  │   │
            │                 │  │ (internal + mirrored plugins) │   │
            │                 │  │ dumb-HTTP git-upload-pack     │   │
            │                 │  └──────────────────────────────┘   │
            │                 └─────────────────┬───────────────────┘
            │                                   │
            │                                   │ (external mode only)
            │                                   ▼
            │                          ┌────────────────────┐
            └─────────────────────────►│ Upstream git host  │
                                       │ (GitHub, GitLab,…) │
                                       └────────────────────┘

                                   │
                                   ▼
                       ┌──────────────────────────┐
                       │ ~/.claude/plugins/cache/ │
                       │   <marketplace>/         │
                       │     <plugin>/            │
                       │       <version>/         │
                       │         .claude-plugin/  │
                       │         skills/, …       │
                       └──────────────────────────┘
                          (client-side, read by
                           every Claude Code
                           session)
```

The two endpoints (`marketplace.json` and the git endpoint) are the entire public surface area Claude Code consumes. Everything else — composer UI, CRUD API, mirror worker, audit logs — is operator-facing.

## Workflows

### Curator — author an internal plugin

**Via the web UI** (most common):

1. Sign in to the portal as a `tenant:admin`.
2. Navigate to **Admin → Plugins → + New plugin**.
3. Fill in slug (kebab-case), display name, version (semver or omit for git-SHA versioning), description.
4. Multi-select skills/agents/commands from the catalogue. Optionally paste inline hook/MCP/monitor JSON blobs.
5. Publish. The plugin appears in `/.claude-plugin/marketplace.json` on the next fetch and is cloneable from the git endpoint immediately.

**Via the CLI:**

```bash
# Build a plugin directory locally that follows the Claude Code layout.
# (See docs/plugin-manifest-schema.md for the full file layout.)
skill-pool plugin publish ./my-plugin/

# Print the marketplace URL to paste into Claude Code.
skill-pool plugin marketplace-url
```

### Curator — list an external plugin

```bash
# Adds an entry to marketplace.json pointing at the upstream git URL.
# No cloning happens server-side. Developers' Claude Code clients must
# be able to reach the upstream URL directly.
skill-pool plugin add-external \
  --name acme-formatter \
  --source-type github \
  --repo acme-corp/formatter
```

### Curator — mirror an external plugin

```bash
# Enqueues a mirror job. When complete, marketplace.json lists the plugin
# with sourcing_mode = mirror and source = skill-pool's git endpoint.
skill-pool plugin import https://github.com/acme-corp/formatter.git \
  --refresh-interval 24h
```

### Developer — install a plugin

```text
# Inside a Claude Code session:
/plugin marketplace add https://acme.skill-pool.example.com
/plugin install rust-axum-toolkit@acme

# To refresh later:
/plugin marketplace update acme
/plugin update rust-axum-toolkit@acme
```

The shell-equivalent for scripting (non-interactive use, container builds, CI) is documented in the Claude Code [CLI commands reference](https://code.claude.com/docs/en/plugins-reference#cli-commands-reference).

### Developer — pin a plugin in a project manifest

`.skill-pool/manifest.toml` gains a `[[plugins]]` block alongside the existing `[[skills]]` / `[[agents]]` / `[[commands]]` blocks. `skill-pool ensure` then resolves each plugin to its bundled contents and installs everything in one pass. See `docs/manifest-schema.md` for the manifest fields once the integration ships (covered by Issue #8 of the plugins epic).

## Authorization

| Path | Scope |
|---|---|
| `GET /v1/plugins` | Any authenticated tenant member |
| `GET /v1/plugins/{slug}` | Any authenticated tenant member |
| `POST /v1/plugins` | `tenant:admin` (curator) |
| `PATCH /v1/plugins/{slug}` | `tenant:admin` |
| `DELETE /v1/plugins/{slug}` | `tenant:admin` |
| `POST /v1/plugins/import` | `tenant:admin` |
| `GET /.claude-plugin/marketplace.json` | Public (no auth) — rate-limited |
| `GET /git/plugins/{slug}.git/...` | Public (no auth) — rate-limited |

Public read on the marketplace endpoint is required by Claude Code's installer: `/plugin marketplace add` is an unauthenticated GET against the URL the user pastes. Tenants that need authenticated marketplaces rely on Claude Code's per-host credential helpers (`GITHUB_TOKEN`, `GITLAB_TOKEN`, `BITBUCKET_TOKEN`) and the standard private-repo flow ([Private repositories](https://code.claude.com/docs/en/plugin-marketplaces#private-repositories)).

## Failure modes

| Symptom | Cause |
|---|---|
| `/plugin marketplace add` returns 404 | Wrong tenant subdomain, or the tenant has never published a plugin (marketplace.json is generated but empty `plugins: []` should still return 200). Check `curl https://<tenant>/.claude-plugin/marketplace.json`. |
| `/plugin install` says "version unchanged" after publish | Plugin manifest declares an explicit `version` and the curator didn't bump it. See [Version resolution](https://code.claude.com/docs/en/plugin-marketplaces#version-resolution-and-release-channels). Bump `version` or omit it to use commit-SHA versioning. |
| Mirrored plugin's git endpoint serves stale content | Refresh worker is failing. Check the warning chip on the plugin detail page; check `last_pulled_at` and the job error in the portal's audit log. |
| Cloned plugin has correct manifest but no skill files | Internal plugin's content references a skill slug whose latest published version was archived after the plugin row was written. Re-publish the plugin (or pin the plugin's content to a specific skill version). |
| External plugin works for some developers but not others | External mode requires every developer's Claude Code client to reach the upstream URL. Switch to mirror mode for air-gapped or restricted-egress developers. |
| Cache survives plugin uninstall | Claude Code keeps `${CLAUDE_PLUGIN_DATA}` after uninstall unless the user passes `--keep-data=false`. See [Persistent data directory](https://code.claude.com/docs/en/plugins-reference#persistent-data-directory). |

## Related

- `docs/plugin-manifest-schema.md` — the exact reference for `.claude-plugin/plugin.json` and the skill-pool-specific rules on top.
- `docs/wiki/Plugin-Authoring.md` — step-by-step composer-to-Claude-Code walkthrough for a first internal plugin.
- `docs/api.md` — the REST endpoints (`/v1/plugins`, `/.claude-plugin/marketplace.json`, `/git/plugins/<slug>.git/...`) behind every workflow on this page.
- `docs/wiki/CLI-Reference.md` — the `skill-pool plugin` subcommand family.
- `docs/architecture.md` — the broader skill-pool system diagram into which the plugin surfaces above slot.
- `docs/tenancy.md` — the `tenant_id` invariant plugins inherit.
- `docs/projects.md` and `docs/plans.md` — the two most recent per-tenant primitives; plugins follow the same RBAC, audit, and refresh-worker patterns.
- [Claude Code: Plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces) — the upstream spec for `marketplace.json` and the install flow.
- [Claude Code: Plugins reference](https://code.claude.com/docs/en/plugins-reference) — the upstream spec for `plugin.json` and the on-disk layout.
