# Wire Claude to the team catalog via MCP

When you're deep in a Claude session and need a skill from the team
catalog, you shouldn't have to switch to a terminal. MCP fixes that.

## What this gives you

Inside any Claude session — Code, Desktop, Web — you can say:

> Find me a team skill about axum tenant extractors

…and Claude calls the `search_skills` tool, fetches the SKILL.md via
`get_skill`, and integrates the pattern straight into its reply. No
context switch.

## 1. Mint a registry token

Any token with at least `skills:read` scope works.

```bash
skill-pool-server admin token-create \
  --tenant acme --name claude-mcp \
  --scope "skills:read"
```

Save the printed `spk_…` string. The portal also has a token mint flow
under **Settings → Members**.

## 2. Add the MCP server to Claude

Wherever your Claude client keeps its MCP config:

```json
{
  "mcpServers": {
    "skill-pool": {
      "type": "http",
      "url": "https://acme.skill-pool.example.com/v1/mcp",
      "headers": {
        "Authorization": "Bearer spk_…",
        "X-Skill-Pool-Tenant": "acme"
      }
    }
  }
}
```

Restart Claude. The tool list should show **`search_skills`**,
**`get_skill`**, **`install_skill`**, and **`get_project_plan`**
under the `skill-pool` server.

## 3. Use it

Mid-conversation, ask:

> Is there a team skill that explains the axum extractor pattern? If so,
> use it before answering.

Claude calls `search_skills(query="axum extractor")`. If a match comes
back ≥ 85% relevance (when semantic search is on the server), it then
calls `get_skill(slug=…)` and reads the SKILL.md before composing its
reply.

## What the server returns

`search_skills` emits two content blocks: a human summary AND a fenced
JSON dump. Claude reads both.

```text
1 matching skill(s):

- axum-handler-tip (94% match) — v1.0.0
  Pattern for axum tenant-scoped extractors that avoids the borrow-checker
  dance with a request-scoped clone.
  when: When building axum handlers that need TenantCtx + AppState.
  tags: rust, axum, tenant
```

```json
[
  {
    "slug": "axum-handler-tip",
    "version": "1.0.0",
    "description": "Pattern for axum tenant-scoped extractors…",
    "similarity": 0.94,
    "tags": ["rust", "axum", "tenant"]
  }
]
```

## Errors are tool-shaped, not protocol-shaped

A missing slug returns `isError: true` inside the tool result so Claude
can recover and ask the user. JSON-RPC errors (`-32601` etc.) are
reserved for protocol breaks: unknown methods, malformed params,
internal failures.

| HTTP / RPC | Meaning | Who fixes it |
|---|---|---|
| HTTP 401 | Missing or invalid bearer token | Update the Claude MCP config |
| RPC -32601 | Unknown method | Bug — open an issue |
| RPC -32602 | Bad arguments to a tool | Claude usually self-corrects |
| `isError: true` | Tool ran but found nothing useful | Claude tells the user |

## Fetching a project plan mid-session

If a developer asks about the current direction of a project, Claude
can call `get_project_plan` to read the active plan without the
developer needing to run `skill-pool ensure` first:

> What's the current plan for acme-billing-service?

Claude calls `get_project_plan(project_slug: "acme-billing-service")`
and uses the returned markdown to answer in context.

## Why only search and plans?

Per the master plan: MCP is a search adapter, not the core transport.
Publishing skills, managing drafts, archiving, theme editing — those
stay on the REST API + portal because they're metadata-heavy and need
human review. Search and read-only plan retrieval are the operations
that benefit from being inside the assistant loop.
