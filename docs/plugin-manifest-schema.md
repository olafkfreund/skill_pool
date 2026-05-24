# Plugin manifest (`plugin.json`)

`<plugin-root>/.claude-plugin/plugin.json` — the manifest Claude Code reads when it installs a plugin. JSON, not TOML. The full spec lives at [Claude Code: Plugin manifest schema](https://code.claude.com/docs/en/plugins-reference#plugin-manifest-schema); this page restates it, names the fields skill-pool reads or writes, and documents the additional rules skill-pool enforces when a manifest is published through its `/v1/plugins` API.

The manifest is optional in Claude Code itself. If omitted, Claude Code auto-discovers components in their [default locations](https://code.claude.com/docs/en/plugins-reference#file-locations-reference) and derives the plugin name from the directory name. **skill-pool requires a manifest** for any plugin published through its API, so the registry has a stable identifier and version to key off.

## Full example

```json
{
  "$schema": "https://json.schemastore.org/claude-code-plugin-manifest.json",
  "name": "rust-axum-toolkit",
  "displayName": "Rust + Axum Toolkit",
  "version": "1.2.0",
  "description": "Curated skills, agents, and hooks for Rust + Axum service development",
  "author": {
    "name": "Acme Platform Team",
    "email": "platform@acme.example.com",
    "url": "https://github.com/acme-corp"
  },
  "homepage": "https://acme.skill-pool.example.com/marketplace/rust-axum-toolkit",
  "repository": "https://acme.skill-pool.example.com/git/plugins/rust-axum-toolkit.git",
  "license": "Apache-2.0",
  "keywords": ["rust", "axum", "backend"],
  "skills": "./skills/",
  "commands": ["./commands/deploy.md"],
  "agents": ["./agents/sqlx-migration-reviewer.md"],
  "hooks": "./hooks/hooks.json",
  "mcpServers": "./.mcp.json",
  "lspServers": "./.lsp.json",
  "experimental": {
    "monitors": "./monitors/monitors.json"
  },
  "dependencies": [
    { "name": "secrets-vault", "version": "~2.1.0" }
  ]
}
```

The same shape, minus the `$schema` and `experimental` fields, is the canonical example in [Claude Code's Complete schema](https://code.claude.com/docs/en/plugins-reference#complete-schema).

## Required fields

If a manifest is present, `name` is the only field Claude Code requires. skill-pool additionally requires `version` and `description` at publish time — see [skill-pool publish-time validation](#skill-pool-publish-time-validation) below.

| Field | Type | Required (Claude Code) | Required (skill-pool publish) | Description |
|---|---|---|---|---|
| `name` | string | yes | yes | Unique plugin identifier. Kebab-case, no spaces. Used for namespacing — an agent `reviewer` in plugin `rust-axum-toolkit` shows as `rust-axum-toolkit:reviewer`. ([Required fields](https://code.claude.com/docs/en/plugins-reference#required-fields)) |

## Metadata fields

All optional in Claude Code. skill-pool reads `version` and `description` at publish time; everything else round-trips through the registry untouched.

| Field | Type | Description | Example |
|---|---|---|---|
| `$schema` | string | JSON Schema URL for editor autocomplete. Ignored at load time. | `"https://json.schemastore.org/claude-code-plugin-manifest.json"` |
| `displayName` | string | Human-readable name for the `/plugin` picker. Falls back to `name`. May contain spaces and any casing. Not used for namespacing. Requires Claude Code v2.1.143+. | `"Rust + Axum Toolkit"` |
| `version` | string | Semver string. **Pins the plugin to this version** — users only receive updates when the string changes. Omit to use the git commit SHA, so every commit is a new version. If set in both `plugin.json` and the marketplace entry, `plugin.json` wins. ([Version management](https://code.claude.com/docs/en/plugins-reference#version-management)) | `"1.2.0"` |
| `description` | string | Brief explanation. | `"Curated skills, agents, and hooks for Rust + Axum"` |
| `author` | object | `{name, email?, url?}`. Author of the plugin. | `{"name": "Platform Team", "email": "platform@acme.example.com"}` |
| `homepage` | string | Documentation URL. | `"https://docs.example.com"` |
| `repository` | string | Source URL. For internal plugins, skill-pool sets this to the tenant's git endpoint. | `"https://github.com/acme/plugin"` |
| `license` | string | SPDX identifier. | `"MIT"`, `"Apache-2.0"` |
| `keywords` | array | Discovery tags. Strings only — Claude Code rejects non-array values at load time. | `["rust", "axum"]` |

## Component path fields

These tell Claude Code where to find each component type inside the plugin's installed directory. Paths must be relative and start with `./`. Some fields *replace* the default directory, some *extend* it — the table flags which.

| Field | Type | Behavior vs default | Default location | Description |
|---|---|---|---|---|
| `skills` | string \| array | **adds to** default `skills/` | `skills/` | Custom skill directories. Each contains `<name>/SKILL.md`. |
| `commands` | string \| array | **replaces** default `commands/` | `commands/` | Flat `.md` files (skills as single files). Use `skills/` for new plugins; `commands/` is the older flat format. |
| `agents` | string \| array | **replaces** default `agents/` | `agents/` | Subagent markdown files. |
| `hooks` | string \| array \| object | own merge rules — see [Hooks](https://code.claude.com/docs/en/plugins-reference#hooks) | `hooks/hooks.json` | Path(s) to JSON hook configs, or an inline `{event: [...]}` object. |
| `mcpServers` | string \| array \| object | own merge rules | `.mcp.json` | Path(s) to MCP config, or an inline `{name: {command, args, env, ...}}` object. |
| `lspServers` | string \| array \| object | own merge rules | `.lsp.json` | Path(s) to LSP config, or an inline `{name: {command, args, ...}}` object. |
| `outputStyles` | string \| array | **replaces** default `output-styles/` | `output-styles/` | Custom output style files or directories. |
| `experimental.themes` | string \| array | **replaces** default `themes/` | `themes/` | Color theme JSONs. Experimental — schema may change between Claude Code releases. |
| `experimental.monitors` | string \| array | **replaces** default `monitors/monitors.json` | `monitors/monitors.json` | Background monitor configs. Experimental. Requires Claude Code v2.1.105+. |
| `userConfig` | object | n/a | n/a | Values Claude Code prompts the user for when the plugin is enabled. See [User configuration](https://code.claude.com/docs/en/plugins-reference#user-configuration). |
| `channels` | array | n/a | n/a | Channel declarations for message-injection plugins (Telegram, Slack-style). See [Channels](https://code.claude.com/docs/en/plugins-reference#channels). |
| `dependencies` | array | n/a | n/a | Other plugins this plugin requires. Each entry is either a string slug or `{name, version}` with semver. See [Constrain plugin dependency versions](https://code.claude.com/docs/en/plugin-dependencies). |

To extend the default `commands/` directory rather than replace it, list it explicitly:

```json
{ "commands": ["./commands/", "./extras/"] }
```

## Filesystem layout

A published plugin's tree, when cloned from skill-pool's git endpoint, looks like ([Standard plugin layout](https://code.claude.com/docs/en/plugins-reference#standard-plugin-layout)):

```text
rust-axum-toolkit/
├── .claude-plugin/
│   └── plugin.json              ← manifest (only file allowed in this dir)
├── skills/                      ← skill directories with SKILL.md
│   ├── rust-axum-handler/
│   │   └── SKILL.md
│   └── sqlx-migrations/
│       └── SKILL.md
├── commands/                    ← flat .md command files
│   └── deploy.md
├── agents/                      ← subagent markdown files
│   └── sqlx-migration-reviewer.md
├── hooks/
│   └── hooks.json
├── .mcp.json                    ← MCP server definitions
├── .lsp.json                    ← LSP server definitions
├── monitors/
│   └── monitors.json
├── output-styles/
│   └── terse.md
├── themes/
│   └── dracula.json
├── bin/                         ← optional: executables added to Bash PATH
│   └── my-tool
├── scripts/                     ← optional: scripts referenced by hooks/MCP
│   └── format.sh
├── LICENSE
└── CHANGELOG.md
```

**Hard rule from the Claude Code spec:** only `plugin.json` belongs inside `.claude-plugin/`. Every other directory (`skills/`, `agents/`, `commands/`, `hooks/`, etc.) must be at the plugin root. Putting components inside `.claude-plugin/` is the single most common authoring mistake — see [Directory structure mistakes](https://code.claude.com/docs/en/plugins-reference#directory-structure-mistakes).

## Path-substitution variables

Available inside hook commands, monitor commands, MCP/LSP configs, and skill/agent content ([Environment variables](https://code.claude.com/docs/en/plugins-reference#environment-variables)):

| Variable | Resolves to | When to use it |
|---|---|---|
| `${CLAUDE_PLUGIN_ROOT}` | Absolute path to the plugin's installed directory (in the cache). Changes on every plugin update. | Reference scripts, binaries, configs bundled inside the plugin. |
| `${CLAUDE_PLUGIN_DATA}` | Persistent directory at `~/.claude/plugins/data/<plugin-id>/` that survives updates. | Installed dependencies (`node_modules`, Python venvs), generated code, caches. |
| `${CLAUDE_PROJECT_DIR}` | The user's project root. Same value as the `CLAUDE_PROJECT_DIR` env var hooks receive. | Reference project-local scripts or configs. |

Wrap each in double quotes when used inside shell-form commands: `"${CLAUDE_PLUGIN_ROOT}"/scripts/format.sh`.

## Versioning

Claude Code resolves a plugin's effective version from the first of these that is set ([Version management](https://code.claude.com/docs/en/plugins-reference#version-management)):

1. `version` in `plugin.json`.
2. `version` in the plugin's marketplace entry.
3. The git commit SHA of the plugin's source (for `github`, `url`, `git-subdir`, and relative-path sources inside a git-hosted marketplace).
4. `unknown` (for `npm` sources or local directories not in a git repo).

**Implication for skill-pool:** internal plugins are served from skill-pool's git endpoint, so they always have a commit SHA available. A curator who wants pinned, human-readable versioning sets `version` in the manifest and bumps it on every release. A curator who wants every commit to ship to users omits `version` and lets the SHA do the work.

The Claude Code spec is explicit: **if you set `version`, you must bump it every release.** Pushing new commits without changing the version string is a no-op for existing users because Claude Code keeps the cached copy.

## skill-pool publish-time validation

When a plugin is published through `POST /v1/plugins`, skill-pool validates the manifest beyond what Claude Code itself enforces. These rules exist to keep the registry consistent and to prevent cross-tenant references.

### Schema rules

- **Required.** `name`, `version`, and `description` must all be set. Claude Code only requires `name`; the extra two fields exist to make the marketplace browser and audit log useful.
- **Slug format.** `name` is kebab-case (`^[a-z0-9]+(-[a-z0-9]+)*$`), 1–64 characters. Matches the Claude.ai marketplace sync rules.
- **Semver.** If `version` is set, it must parse as semver per [semver.org](https://semver.org).
- **Body size.** The full manifest plus contents block must be under 256 KB.
- **Unique within tenant.** `(tenant_id, name, version)` is unique. Republishing the same `name`+`version` is rejected; bump `version` or archive the old row first.

### Cross-content rules

A published plugin's `contents` block — separate from the manifest, posted alongside it — lists which skills/agents/commands the plugin bundles by registry slug:

```json
{
  "manifest": {
    "name": "rust-axum-toolkit",
    "version": "1.2.0",
    "description": "Curated skills, agents, and hooks for Rust + Axum"
  },
  "contents": [
    { "kind": "skill",   "slug": "rust-axum-handler" },
    { "kind": "skill",   "slug": "sqlx-migrations" },
    { "kind": "agent",   "slug": "sqlx-migration-reviewer" },
    { "kind": "command", "slug": "deploy" }
  ]
}
```

skill-pool validates:

- **Tenant scope.** Every slug in `contents[]` must resolve to a published row in the *same tenant* as the publishing user. Cross-tenant references are rejected (HTTP 422).
- **Kind matches.** `kind` must match the table the slug lives in. A slug registered under `agents` can't be referenced as a `skill`.
- **Published state.** The referenced skill/agent/command must be in a published (non-archived) state at the time of plugin publish. If an item is later archived, the plugin keeps serving the old content from its frozen git tree, but `/v1/plugins/{slug}` flags it in the response payload so the portal can warn.
- **Cap.** No more than 64 items in `contents[]`. Larger plugins are usually a smell — split or reconsider.

These rules don't appear in the Claude Code spec because Claude Code's installer doesn't know about skill-pool's registry. They're the contract between the registry and a published plugin row.

### Reserved names

Claude Code's spec also reserves a small set of marketplace names for official Anthropic use ([Reserved names](https://code.claude.com/docs/en/plugin-marketplaces#required-fields)). skill-pool's marketplace name is derived from the tenant slug and a configured suffix, so it never collides — but custom marketplace names (if a future feature exposes them) would need to filter against that list.

## Field cheatsheet

For copy-paste readers — the same data as the tables above, in one place:

```text
Required:                  name
Required at publish:       name, version, description
Metadata (all optional):   $schema, displayName, version, description, author,
                           homepage, repository, license, keywords
Component paths:           skills, commands, agents, hooks, mcpServers,
                           lspServers, outputStyles,
                           experimental.themes, experimental.monitors
Behavior:                  userConfig, channels, dependencies
Path rules:                relative, start with ./, no ../ traversal
Default dirs:              skills/, commands/, agents/, hooks/hooks.json,
                           .mcp.json, .lsp.json, monitors/monitors.json,
                           output-styles/, themes/, bin/, settings.json
Manifest must live at:     .claude-plugin/plugin.json
Components must NOT be:    inside .claude-plugin/
```

## Failure modes

| Symptom | Cause |
|---|---|
| `POST /v1/plugins` returns 422 with `validation: cross_tenant_content` | The `contents[]` block references a slug published in a different tenant. Republish from the tenant that owns the content, or have that tenant publish the plugin. |
| `POST /v1/plugins` returns 422 with `validation: version_required` | skill-pool requires `version` at publish time, unlike Claude Code itself. Add `"version": "<semver>"` to the manifest. |
| `POST /v1/plugins` returns 409 | Duplicate `(tenant, name, version)`. Bump the manifest's `version` or archive the existing row. |
| Plugin installs but skills don't show up | Almost always the components-inside-`.claude-plugin/` mistake. Move them to the plugin root. ([Directory structure mistakes](https://code.claude.com/docs/en/plugins-reference#directory-structure-mistakes)) |
| Plugin installs but `${CLAUDE_PLUGIN_ROOT}` resolves to an empty path in a hook | The hook command isn't quoting the variable. Use `"${CLAUDE_PLUGIN_ROOT}"/scripts/x.sh` in shell form, or the exec-form variant with `args`. ([Environment variables](https://code.claude.com/docs/en/plugins-reference#environment-variables)) |
| `claude plugin validate` warns about unrecognized fields | Manifest carries metadata from another ecosystem (VS Code, npm). Claude Code ignores these at load time; pass `--strict` in CI to fail on them. |
| `/plugin update` reports "already at latest" after a republish | The manifest's `version` didn't change. Either bump it, or omit `version` so the git SHA is used instead. |

## Related

- `docs/plugins.md` — conceptual overview, sourcing modes, how plugins fit alongside skills/agents/commands.
- `docs/manifest-schema.md` — the project-level `.skill-pool/manifest.toml`, a separate TOML manifest unrelated to `plugin.json`.
- `docs/wiki/Plugin-Authoring.md` — step-by-step composer-to-Claude-Code walkthrough.
- `docs/api.md` — REST endpoints (`/v1/plugins`, `/.claude-plugin/marketplace.json`, `/git/plugins/<slug>.git/...`) that consume and serve this manifest.
- [Claude Code: Plugins reference](https://code.claude.com/docs/en/plugins-reference) — upstream spec for `plugin.json` and the on-disk layout.
- [Claude Code: Plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces) — upstream spec for `marketplace.json` and the install flow.
- [Claude Code: Plugin dependencies](https://code.claude.com/docs/en/plugin-dependencies) — semver constraints and resolution.
