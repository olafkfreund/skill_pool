# Plugin Authoring

> First-internal-plugin walkthrough: compose in the portal, publish,
> copy the marketplace URL, install in Claude Code with
> `/plugin marketplace add` + `/plugin install`. End-to-end in under
> five minutes.

This page is the happy-path tutorial. For the conceptual model (what
plugins are vs skills/agents/commands, the three sourcing modes,
per-tenant marketplace shape) read [`docs/plugins.md`](../plugins.md)
first. For the exact `plugin.json` fields skill-pool reads and the
publish-time validation rules, see
[`docs/plugin-manifest-schema.md`](../plugin-manifest-schema.md).
For the REST endpoints behind every step, see
[`docs/api.md`](../api.md). For the CLI flags, see
[`CLI-Reference.md`](CLI-Reference.md).

## Prerequisites

- You are a member of a tenant on a running skill-pool instance and
  signed in to its portal as a `tenant:admin` (curators publish
  plugins; see [`docs/plugins.md#authorization`](../plugins.md#authorization)).
- The tenant already has at least one **published** skill, agent, or
  command in the catalogue. A plugin bundles existing published rows;
  it does not create them. (To publish your first one, see
  [`CLI-Reference.md#publish`](CLI-Reference.md#publish).)
- Claude Code v2.1.105 or newer on the consuming developer's machine
  (older versions don't speak the full marketplace schema). The
  `claude --version` field reports it.

## Step 1 — Open the portal's Plugins surface

In your tenant's portal, navigate to **Admin → Plugins**:

```text
https://acme.skill-pool.example.com/admin/plugins
```

You should see the tenant's existing plugins (empty list on first
visit) and three actions in the header:

- **+ New plugin** — opens the composer for an internal plugin.
- **+ Import** — registers an external git URL (`external` or
  `mirror` sourcing mode).
- **Marketplace URL** — copies the `/plugin marketplace add` URL to
  the clipboard (same URL `skill-pool plugin marketplace-url` prints).

For this walkthrough click **+ New plugin**.

## Step 2 — Fill in the manifest

The composer at `/admin/plugins/new` collects the four required
fields for a publish:

| Field | Required | Example | Source-of-truth field |
|---|---|---|---|
| Slug | yes | `rust-axum-toolkit` | `slug` (registry-side identifier; doubles as `plugin.json#name`). Kebab-case, 1–64 chars. |
| Version | yes | `1.0.0` | `manifest.version`. Semver. **Bump on every publish** — pinned plugins ignore identical strings (see [Version management](https://code.claude.com/docs/en/plugins-reference#version-management)). |
| Display name | no | `Rust + Axum Toolkit` | `manifest.displayName`. Falls back to slug. |
| Description | yes | `Curated skills, agents, and hooks for Rust + Axum.` | `manifest.description`. Required by skill-pool (Claude Code itself doesn't require it). |

Below the manifest section, pick **Sourcing mode**:

- **Internal (composed here)** — skill-pool will assemble the plugin
  tree from your selected catalogue items and host it from
  `/git/plugins/<slug>.git`. This is the default and the path this
  walkthrough takes.
- **External (upstream git, no mirror)** — you paste the upstream
  git URL; Claude Code clones directly from it. Developers must reach
  the upstream host.
- **Mirror (clone + serve locally)** — skill-pool clones the upstream
  on a schedule and serves the mirror from its own git endpoint.
  Pull-job worker tracked in a follow-up issue.

Leave the radio on **Internal** for this walkthrough.

## Step 3 — Pick what to bundle

Below the Manifest section the composer renders a three-column picker
(skills / agents / commands). For each column you can:

- Search the tenant's catalogue by slug substring.
- Click an item to add it to the plugin's `contents[]`.
- Remove a selected item with the per-row X button.

The counter under the **Contents** heading shows
`<selected> of 64 max`. The 64-item cap is documented in
[`docs/plugin-manifest-schema.md#cross-content-rules`](../plugin-manifest-schema.md#cross-content-rules);
larger plugins are usually a sign you want two plugins.

Pick at least one published item from any column. The **Publish
plugin** submit button stays disabled until you do.

> **Tenant scope.** Every item must be published in **this** tenant.
> Cross-tenant references are rejected at publish time with HTTP 422
> (see [`docs/api.md#post-v1plugins--publish`](../api.md#post-v1plugins--publish)).

## Step 4 — Add optional inline blobs

Below the contents picker the composer has textareas for inline
config — paste JSON for any of:

- **Hooks** — `hooks.json` body (per
  [Claude Code hooks reference](https://code.claude.com/docs/en/plugins-reference#hooks)).
- **MCP servers** — `.mcp.json` body
  ([MCP servers reference](https://code.claude.com/docs/en/plugins-reference#mcp-servers)).
- **LSP servers** — `.lsp.json` body
  ([LSP servers reference](https://code.claude.com/docs/en/plugins-reference#lsp-servers)).
- **Monitors** (experimental) —
  [monitors reference](https://code.claude.com/docs/en/plugins-reference#monitors-experimental).

Each field validates as JSON before submit. Leave all four empty for a
plain skills-and-agents bundle.

## Step 5 — Publish

Click **Publish plugin**. The form posts to the server (which fronts
[`POST /v1/plugins`](../api.md#post-v1plugins--publish)) and on
success redirects to the plugin's detail page at
`/admin/plugins/<slug>`. Two server-side side effects fire on success:

1. The bare git repo is materialised under the tenant's storage
   (`<state-dir>/.../plugins/<slug>.git/`). Operators see this
   directory grow over time — see
   [`Operator-Guide.md#plugin-storage`](Operator-Guide.md#plugin-storage).
2. A row is upserted into `plugin_marketplace_entries` so the next
   fetch of `/.claude-plugin/marketplace.json` surfaces the plugin.

Common publish errors and how they surface:

| Symptom | Cause | Fix |
|---|---|---|
| 422 with `name: required and non-empty` | Slug empty or manifest missing the field. | Fill in the slug (which doubles as `manifest.name`). |
| 422 listing `contents[i]` as `not published in this tenant` | One of the picked items was archived after the page loaded, or you pasted a slug from another tenant. | Refresh the picker; re-select. |
| 409 with `plugin <slug>@<version> already exists` | Republishing the exact same `(slug, version)`. | Bump `version` and resubmit. |
| 413 with `manifest is N bytes; limit is 262144` | Manifest body (with inline hooks/MCP blobs) exceeds the 256 KiB cap. | Move large blobs to bundled skill files; reference them with relative paths instead of inlining. |

## Step 6 — Copy the marketplace URL

The shape Claude Code's `/plugin marketplace add` expects is:

```text
https://<tenant>.<skill-pool-host>/.claude-plugin/marketplace.json
```

Three ways to get the right URL for your tenant:

1. **Portal** — Admin → Plugins → "Marketplace URL" button (top right).
2. **CLI**:

   ```bash
   skill-pool plugin marketplace-url
   # https://acme.skill-pool.example.com/.claude-plugin/marketplace.json
   ```

3. **Hand-roll** by gluing `<tenant-slug>` onto your registry host
   (the CLI does exactly this — see
   `cli/src/cmd/plugin.rs:303-322` and
   [`CLI-Reference.md#plugin-marketplace-url`](CLI-Reference.md#plugin-marketplace-url)).

Verify the URL serves a valid catalogue:

```bash
curl -sS https://acme.skill-pool.example.com/.claude-plugin/marketplace.json | jq '.plugins[].name'
# "rust-axum-toolkit"
```

A 200 with an empty `plugins: []` array is also normal — it just means
no plugin has been published yet in this tenant. A 404 means you got
the tenant subdomain wrong.

## Step 7 — Install in Claude Code

Inside any Claude Code session — interactive REPL or `--print` —
register the marketplace once per machine:

```text
/plugin marketplace add https://acme.skill-pool.example.com/.claude-plugin/marketplace.json
```

Claude Code fetches the JSON, validates it against the spec, and
registers the marketplace under the name in the JSON's `name` field
(your tenant slug by default — `acme` in the example above). The
prompt confirms before saving; pick your install scope per the
[plugin installation scopes](https://code.claude.com/docs/en/plugins-reference#plugin-installation-scopes)
docs (user vs project).

Now install the plugin you just published:

```text
/plugin install rust-axum-toolkit@acme
```

Claude Code reads the entry's `source.url` from `marketplace.json`,
`git clone`s it (against your skill-pool's `/git/plugins/...`
endpoint for `internal` plugins), and extracts it into:

```text
~/.claude/plugins/cache/acme/rust-axum-toolkit/<version>/
```

The plugin's skills/agents/commands light up at the **next session
start** — Claude Code only re-scans the cache on session boot. Open
a new session (`/quit` + relaunch) and your new skills appear in the
`/skills` list, agents in `/agents`, and any commands as `/<command-name>`.

## Step 8 — Verify and iterate

A quick three-line sanity check inside Claude Code:

```text
/plugin                       # list installed plugins; yours should appear
/skills                       # the bundled skills should be listed
/<your-command-name>          # if you bundled a command, invoking it should work
```

To ship a new version after editing one of the bundled items:

1. Bump that skill/agent/command in its own publish step
   (`skill-pool publish ./skill-dir --version <new>` — see
   [`CLI-Reference.md#publish`](CLI-Reference.md#publish)).
2. Go back to the portal's `/admin/plugins/<slug>` and click
   **Publish new version** — re-pick the same items with the new
   versions and bump `Version`.
3. On the developer side, run `/plugin marketplace update acme`
   followed by `/plugin update rust-axum-toolkit@acme`. Claude Code
   re-fetches `marketplace.json`, sees the higher version, and
   re-clones.

## CLI alternative

The whole flow has a CLI equivalent for non-interactive use (CI
pipelines, container builds, scripted onboarding):

```bash
# 1. Assemble a plugin directory locally following the standard layout
#    (see docs/plugin-manifest-schema.md#filesystem-layout).
#    The directory needs .claude-plugin/plugin.json plus the bundled
#    skill/agent/command files at the plugin root.

# 2. Publish.
skill-pool plugin publish ./my-plugin/

# 3. Copy the marketplace URL.
skill-pool plugin marketplace-url

# 4. Install in Claude Code (still interactive — Claude Code's
#    install commands are slash commands, not shell commands).
```

The CLI's `publish` does the same local validation as the portal,
then POSTs to `/v1/plugins`. See
[`CLI-Reference.md#plugin-publish`](CLI-Reference.md#plugin-publish)
for the full flag list and exit codes.

## Troubleshooting

| Symptom | What to check |
|---|---|
| `/plugin marketplace add` reports `404 Not Found` | The tenant subdomain is wrong, or the host doesn't have any tenant configured for that subdomain. `curl -i` the URL — a 200 with `{"plugins": []}` is fine, a 404 is wrong. |
| `/plugin install` clones successfully but the skills don't appear | Almost always the components-inside-`.claude-plugin/` mistake. Move them to the plugin root. See [Directory structure mistakes](https://code.claude.com/docs/en/plugins-reference#directory-structure-mistakes). |
| `/plugin install` reports "already at latest" after a republish | Manifest `version` didn't change. Bump it or omit it (so the git SHA is used) — see [Version management](https://code.claude.com/docs/en/plugins-reference#version-management). |
| Internal plugin's git clone returns 404 | Internal-mode materialisation failed silently at publish time (logged warning, no API failure). Republish to retry. |
| Mirror-sourced plugin serves stale content | Mirror refresh worker isn't pulling. Check the plugin detail page in the portal for the warning chip; check `last_pulled_at`. (Pull-job worker ships in a follow-up issue.) |
| Cache survives an uninstall | Claude Code keeps `${CLAUDE_PLUGIN_DATA}` after uninstall by default. Pass `--keep-data=false` to wipe — see [Persistent data directory](https://code.claude.com/docs/en/plugins-reference#persistent-data-directory). |

## Related

- [`docs/plugins.md`](../plugins.md) — conceptual overview, sourcing
  modes, per-tenant marketplace shape.
- [`docs/plugin-manifest-schema.md`](../plugin-manifest-schema.md) —
  every `plugin.json` field, with skill-pool's publish-time validation
  on top.
- [`docs/api.md`](../api.md) — REST endpoints behind each step
  (publish, list, marketplace.json, git endpoint).
- [`CLI-Reference.md`](CLI-Reference.md) — `skill-pool plugin`
  subcommand family.
- [Claude Code: Plugin marketplaces](https://code.claude.com/docs/en/plugin-marketplaces)
  — upstream spec for `marketplace.json` and the install flow.
- [Claude Code: Plugins reference](https://code.claude.com/docs/en/plugins-reference)
  — upstream spec for `plugin.json` and the on-disk layout.
