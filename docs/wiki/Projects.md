# Projects

> Per-tenant, per-codebase curated bundles of skills, agents, and commands.

Projects are the answer to **"what does *this specific* codebase need from the registry?"** — different from stack mappings ("what does *any Rust+Axum project* need?"). They coexist: when both fire, projects take precedence and stack mappings backfill remaining slots up to the existing 8-result cap.

## When to use Projects

| You want… | Use… |
|---|---|
| "Any Rust+Axum repo should install code-reviewer + sqlx-migrations" | Stack mapping |
| "The Acme Billing Service repo should install exactly these 6 skills + 2 agents" | Project |
| Both: project-specific defaults plus stack-driven backfill | Project (it auto-merges with stack mappings) |

## Model

| Table | Holds |
|---|---|
| `tenant_projects(id, slug, name, description, git_remote, stack_tags, created/updated_at)` | One row per project |
| `tenant_project_items(project_id, skill_slug, kind, position)` | Ordered item list — kind ∈ {skill, agent, command} |

A project's slug is unique within the tenant. `git_remote` is optional but enables CLI auto-discovery (see below).

## Workflows

### Curator — create and curate a project

**Via the web UI** (most common):
1. Sign in to the portal as a `tenant:admin`
2. Navigate to **Settings → Projects**
3. Click **+ New project**
4. Fill in slug (kebab-case, e.g. `acme-billing-service`), name, optional description, optional git remote URL
5. On the detail page: add items via the three sub-tables (Skills / Agents / Commands). Order matters — items install in the listed sequence.

**Via the API** (CI integration, scripted setup):
```bash
# Token must have `tenant:admin` scope.
TOKEN=spk_…
TENANT=acme

# Create
curl -X POST https://your-server/v1/tenant/projects \
  -H "Authorization: Bearer $TOKEN" \
  -H "x-skill-pool-tenant: $TENANT" \
  -H "Content-Type: application/json" \
  -d '{
    "slug": "acme-billing-service",
    "name": "Acme Billing Service",
    "description": "Internal billing service",
    "git_remote": "https://github.com/acme/billing-service"
  }'

# Replace item list (full replacement; preserves order)
curl -X PUT https://your-server/v1/tenant/projects/acme-billing-service/items \
  -H "Authorization: Bearer $TOKEN" \
  -H "x-skill-pool-tenant: $TENANT" \
  -H "Content-Type: application/json" \
  -d '[
    {"slug": "code-reviewer", "kind": "skill"},
    {"slug": "api-design-reviewer", "kind": "skill"},
    {"slug": "cs-backend-engineer", "kind": "agent"},
    {"slug": "/deploy", "kind": "command"}
  ]'
```

### Developer — onboard to a curated project

**Option A — explicit (works without git):**
```bash
cd my-project
skill-pool init --project acme-billing-service
skill-pool bootstrap --yes
```
`init --project` writes `[project] slug = "acme-billing-service"` into `.skill-pool/manifest.toml`. `bootstrap` then queries `/v1/bootstrap?project=acme-billing-service` and installs the project's items.

**Option B — git-remote auto-discovery (zero-config for fresh clones):**
```bash
git clone git@github.com:acme/billing-service.git
cd billing-service
skill-pool bootstrap --yes
```
`bootstrap` runs `git config --get remote.origin.url`, hits `/v1/projects/resolve?remote=<url>`, and if the server matches it to a project, installs that bundle. On success the slug + URL are pinned into `.skill-pool/manifest.toml` so subsequent runs skip the git shell-out.

**Option C — pin after the fact:**
```bash
cd existing-project
skill-pool project link acme-billing-service   # writes slug into manifest
skill-pool ensure                              # installs the bundle
```

### Discover projects from the terminal

```bash
skill-pool project list                   # all projects in the registry
skill-pool project show <slug>            # one project + its items
skill-pool project unlink                 # clear the slug from manifest
```

## Bootstrap precedence

When a project resolves, `GET /v1/bootstrap?project=<slug>&stack=<tags>` returns:

| Tier | Source | Order |
|---|---|---|
| 0 (project) | Curated items from `tenant_project_items` | Preserved (by `position`) |
| 1 (curated) | `tenant_stack_mappings` for any tag in `?stack=` | First-match, dedup against tier 0 |
| 2 (tagged) | Skills whose `tags[]` overlap the stack tags | Ranked by overlap count |
| 3 (semantic) | Skills ranked by cosine similarity of `description_embedding` | Filled last |

All four tiers union, dedup, and cap at **8 total**. Project items always lead.

In `?debug=1` mode the response's `tier_breakdown` reports each tier's contributing slugs separately.

## Git remote normalization

To make remote matching robust across the SSH/HTTPS/`.git`-suffix variants, the server normalizes remote URLs before storage and lookup. Equivalent inputs:

| Input | Normalized |
|---|---|
| `git@github.com:acme/billing.git` | `https://github.com/acme/billing` |
| `https://github.com/acme/billing.git` | `https://github.com/acme/billing` |
| `https://GITHUB.com/acme/billing/` | `https://github.com/acme/billing` |

Path case is preserved (some hosts are case-sensitive); scheme + host are lowercased.

## Authorization

| Path | Scope |
|---|---|
| `GET /v1/tenant/projects` | `tenant:admin` |
| `POST /v1/tenant/projects` | `tenant:admin` |
| `GET /v1/tenant/projects/{slug}` | `tenant:admin` |
| `PATCH /v1/tenant/projects/{slug}` | `tenant:admin` |
| `DELETE /v1/tenant/projects/{slug}` | `tenant:admin` |
| `PUT /v1/tenant/projects/{slug}/items` | `tenant:admin` |
| `GET /v1/projects/resolve?remote=<url>` | Any authenticated tenant member |
| `GET /v1/bootstrap?project=<slug>` | Any authenticated tenant member |

`resolve` and the bootstrap query are intentionally non-admin so developer CLI flows work with their personal scoped tokens.

## Manifest schema

`.skill-pool/manifest.toml` gains two optional fields:

```toml
[project]
stack = ["rust", "axum", "postgres"]
slug = "acme-billing-service"             # NEW: curator-pinned project ID
remote = "https://github.com/acme/billing-service"  # NEW: cached git remote URL
tenant = "acme"                            # existing
```

Both are `Option<String>`, omitted from serialization when None. Existing manifests parse unchanged.

## Failure modes

| Symptom | Cause |
|---|---|
| `bootstrap` installs nothing despite `project.slug` set | The project doesn't exist (server returns soft-empty), check `skill-pool project list` |
| Project items are not respected | Token lacks `tenant:admin` on resolve flow (rare — `resolve` doesn't need admin; check token scopes) |
| Git auto-discovery fails | `git` not on PATH, no `origin` remote, or stored `git_remote` doesn't normalize-equal to repo's origin |
| Items appear in wrong order | The `PUT /items` call replaces the full list — re-PUT with the desired order |

## Related

- `docs/bootstrap.md` — full bootstrap algorithm including the curated/tagged/semantic tiers
- `docs/manifest-schema.md` — the `.skill-pool/manifest.toml` format
- `docs/tenancy.md` — the `tenant_id` invariant projects share with the rest of the schema
- `docs/wiki/Tenant-Onboarding.md` — first-project walkthrough as part of the broader playbook
