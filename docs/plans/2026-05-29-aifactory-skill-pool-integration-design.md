# AIFactory ↔ skill_pool integration — design

> **Spec:** aifactory-skill-pool-integration
> **Created:** 2026-05-29
> **Status:** Approved (design phase)
> **Source:** super-brainstorm session

---

## Summary

AIFactory is a spec-driven planner → coder → QA agent pipeline that turns a GitHub issue into a merge-ready branch. skill_pool is a self-hosted multi-tenant registry of curated Claude Code skills, agents, commands, and plugins, scoped per project. This design wires them together so that **every AIFactory task automatically inherits the curated skill bundle the same team uses in human Claude Code sessions** — one source of truth for "how we work in this repo," applied identically to autonomous overnight runs.

The integration is **prompt-time injection driven by git-remote auto-discovery**: when AIFactory starts a task on a worktree, it asks the team's skill_pool tenant *"do you have a project for this git remote?"*, pulls the matching plan + its skills, and concatenates each skill's content into the relevant phase prompt (`planner.md`, `coder.md`, `qa_reviewer.md`) based on a `phase:` frontmatter tag. Results are cached and pinned in the AIFactory spec record so re-runs are reproducible.

## Why this exists

AIFactory and skill_pool target two ends of the same pipeline:

- **skill_pool is the curation layer** — what skills/agents/commands a team has agreed are good, scoped per repo, governed by RBAC + audit, capturable from real work.
- **AIFactory is the execution layer** — what those skills become when an autonomous pipeline runs the work end-to-end.

Today these layers don't talk. A team can spend months curating an excellent per-repo skill bundle for their human Claude Code users (`code-reviewer`, `sqlx-migrations`, `axum-tracing`, etc.), then AIFactory's autonomous coder runs on the same repo using only its built-in `prompts/coder.md` — none of that curation reaches the autonomous run. Conversely, AIFactory's planner produces excellent specs that never feed back into the team's skill catalog.

This design closes the first gap. Future deliverables (out of scope here) close the second.

**Developer benefit, concretely.**
- A curator pins `code-reviewer@1.4 + sqlx-migrations@2.0 + axum-tracing@0.3` to the `acme-billing-service` project in skill_pool. Effort: one-time.
- Every developer who clones the repo and runs `direnv allow` gets that bundle into `.claude/skills/` — they get human-Claude-Code value automatically (existing skill_pool feature).
- **Now also**: every AIFactory task on that repo — including overnight `/handover` runs — has those same skills injected into its planner / coder / QA prompts. The curator's effort scales identically across human and autonomous workflows.
- Re-runs of the same AIFactory spec produce identical agent behavior because the resolved skill versions are pinned in the spec record.

## In scope (v1)

- AIFactory's planner, coder, and QA phases all consume skill_pool skills.
- Auto-discovery by git remote URL, with an explicit-pin override.
- Per-AIFactory-project read-scoped skill_pool token, encrypted at rest (same mechanism as portal-managed-git-clones PATs).
- Cache + version-pin on first run; soft-fail if skill_pool is unreachable on first run; cache-served re-runs survive subsequent outages.
- New documentation on skill_pool side formalizing the `phase: planner|coder|qa|all` frontmatter tag.
- No new skill_pool API endpoints. Existing `/v1/projects` + `/v1/skills/{slug}/skill-md` cover the read path.

## Out of scope (deferred to later work)

- **Factory → Registry capture.** Posting successful AIFactory transcripts to skill_pool's `/v1/drafts` for the Haiku→Sonnet drafter. Own deliverable.
- **MCP peer-bridge.** Cross-exposing skill_pool's `search_skills` / `install_skill` to AIFactory's 27-tool MCP surface. Own deliverable.
- **AIFactory packaged as a skill_pool plugin.** Distribution play, not runtime integration. Own deliverable.
- **Semantic ranking** of skills to phase. Frontmatter tag is the v1 routing primitive.
- **Per-developer auth.** Per-AIFactory-project tokens cover both attended and autonomous runs; per-developer tokens break the `/handover` overnight flow.
- **OIDC service-to-service.** v2 territory; both sides need plumbing not present today.
- **Bundle compression / partial injection.** Hard size cap (50 total / 20 per phase) is sufficient for v1; if real usage shows pressure, revisit.

## Decisions (the seven forks)

| # | Decision | What we picked | Why |
|---|---|---|---|
| 1 | Direction | skill_pool → AIFactory | Highest immediate leverage: curators already pin per-repo bundles; autonomous runs inherit them for free. |
| 2 | Injection mechanism | Prompt-time concatenation in `prompts_pkg/prompt_generator.py` | Works across all AIFactory providers (Anthropic, OpenAI, Ollama, Gemini, Codex) because it's text. Per-phase control. `project_dir` already an arg of the generator functions. |
| 3 | Resolution key | Auto-discover by git remote, override pin | Matches skill_pool's existing `bootstrap` discovery semantics. Zero config for 80% case, escape hatch for monorepos/forks/dual-named repos. |
| 4 | Phase routing | `phase: planner\|coder\|qa\|all` frontmatter tag (default `all` if absent) | Reuses skill_pool's existing free-form tag/frontmatter system. No schema change. Reversible if convention evolves. |
| 5 | Auth | Per-AIFactory-project read-scoped skill_pool token, stored on a sidecar `SkillPoolProjectConfig` ORM table with the existing `EncryptedString` column type (`apps/web-server/server/crypto/encrypted_string.py`) — same pattern as `EmailAccount.access_token`, `LlmEndpoint.api_key`, `ApiKey.token` | Survives developer logoff (required for overnight `/handover`). Read scope = narrow blast radius. `EncryptedString` is covered by `crypto/rotation.py` automatically. Sidecar table avoids entangling with `routes/projects.py`'s JSON-file storage. |
| 6 | Reliability | Cache + pin-at-first-run + soft-fail on first-run-unreachable | Aligns with AIFactory's spec-first ethos (skills become part of the spec record) and reliability ethos (no catastrophic halt). Re-runs are reproducible from cache. |
| 7 | v1 scope | All three phases (planner + coder + QA) | Plumbing (token, REST client, resolver, cache, pin) is the same regardless of phase count. Per-phase routing is ~30 LOC. Real value is consistency across phases. |

## Architecture

```mermaid
flowchart LR
    subgraph dev[Developer or Console]
        UI[Frontend Settings]
        Trig["AIFactory task start<br/>(human / /handover)"]
    end

    subgraph aif[AIFactory backend]
        Resolver["integrations.skill_pool<br/>resolver"]
        Client["integrations.skill_pool<br/>client (httpx)"]
        Cache["integrations.skill_pool<br/>cache"]
        Injector["prompts_pkg.skill_pool_injector"]
        PG["prompts_pkg.prompt_generator<br/>(planner / coder / QA)"]
        ProjModel["project.models<br/>(+ skill_pool fields)"]
        Specrec[SpecRecord]
        KMS[(KMS · existing)]
    end

    subgraph sp[skill_pool backend]
        REST["REST /v1/*<br/>(unchanged)"]
        PG2[(Postgres + pgvector)]
    end

    UI -->|paste token / override| ProjModel
    ProjModel -.token ref.-> KMS
    Trig --> Resolver
    Resolver --> Cache
    Resolver -->|cache miss| Client
    Client -->|Bearer| REST
    REST --> PG2
    Client --> Cache
    Cache --> Resolver
    Resolver -->|pinned bundle| Specrec
    Resolver -->|bundle| Injector
    PG -->|inject(phase)| Injector
    Injector -->|fenced block + base prompt| PG
    PG -->|final prompt| Trig
```

**Two halves.**

*AIFactory side (new code).*
- `apps/backend/integrations/skill_pool/` — new module sibling to existing `bmad/` and `graphiti/`. Five files: `__init__.py`, `client.py`, `resolver.py`, `cache.py`, `models.py`, plus a `README.md`.
- `apps/backend/prompts_pkg/skill_pool_injector.py` — new sibling so the existing prompt-building functions stay short.
- **Two prompt-building files get hook edits, one site each:**
  - `apps/backend/prompts_pkg/prompt_generator.py` → edit `generate_planner_prompt(spec_dir, project_dir)` (planner phase) and `generate_subtask_prompt(...)` (coder phase, called per subtask).
  - `apps/backend/prompts_pkg/prompts.py` → edit `get_qa_reviewer_prompt(spec_dir, project_dir)` (QA phase).
- `apps/web-server/server/database/models.py` — new **`SkillPoolProjectConfig` sidecar ORM table** (not columns on `Project`, because `routes/projects.py` is JSON-file-backed today — see Settings model section for the why). Encrypts the token via the existing `EncryptedString` pattern.
- One Alembic migration under `apps/web-server/server/database/alembic/versions/`, following the existing date-prefixed filename convention: `<YYYYMMDD>_<rev>_add_skill_pool_project_configs.py`.
- `apps/frontend-web` — new section in **Settings → Integrations**: paste field for token + override pin field + a "test connection" button hitting `GET /v1/healthz`. Backed by three new routes in `apps/web-server/server/routes/projects.py` (`PUT`/`GET`/`DELETE /projects/{projectId}/skill-pool-config`).

*skill_pool side (no API change).*
- New page `docs/wiki/Phase-Tag-Convention.md` formalizing `phase: planner|coder|qa|all`.
- New page `docs/integrations/aifactory.md` — operator-facing guide on issuing the AIFactory token + curating phase-tagged skills.
- One new server-side test in `server/tests/aifactory_phase_tag.rs` that round-trips a phase-tagged skill through publish + fetch to prevent silent regression.

## Components in detail

### Verified hook points (AIFactory, 2026-05-29)

| Phase | File | Function | Signature (verified against source on 2026-05-29) |
|---|---|---|---|
| Planner | `apps/backend/prompts_pkg/prompt_generator.py` | `generate_planner_prompt` | `(spec_dir: Path, project_dir: Path \| None = None) -> str` |
| Coder | `apps/backend/prompts_pkg/prompt_generator.py` | `generate_subtask_prompt` | `(spec_dir: Path, project_dir: Path, subtask: dict, phase: dict, attempt_count: int = 0, recovery_hints: list[str] \| None = None) -> str` |
| QA | `apps/backend/prompts_pkg/prompts.py` | `get_qa_reviewer_prompt` | `(spec_dir: Path, project_dir: Path) -> str` |

The injector signature (`inject_skill_pool(base_prompt, phase, project_dir, spec_record)`) takes only what every hook point already has, so each hook becomes a single-line wrap of the existing return value. There is also a legacy `get_coding_prompt(spec_dir)` in `prompts.py` — verify it is dead in production before deciding whether to wrap it; if still routed to by any caller, add a fourth hook in the same form.

### skill_pool endpoints AIFactory calls

All exist today on Bearer-scoped `/v1` (see [`docs/api.md`](../api.md)):

| Method | Path | Purpose | Returns |
|---|---|---|---|
| GET | `/v1/healthz` | Settings page "test connection" | `{status, version, deps.*}` |
| GET | `/v1/projects/resolve?remote=<url>` | Resolve worktree → project | `{slug, current_plan_id, …}` or 404 |
| GET | `/v1/tenant/projects/{slug}/plan` | Current pinned plan | `{plan_id, items: [{slug, kind, version, tags}]}` |
| GET | `/v1/skills/{slug}/skill-md?version=<v>&kind=<k>` | Raw `SKILL.md` w/ frontmatter | text/markdown |

Verified against `server/src/routes/mod.rs` and `server/src/routes/projects.rs` on 2026-05-29 — path / query-param shapes are exact.

A convenience endpoint `GET /v1/tenant/projects/{slug}/bundle?phase=<p>` for server-side filtering is recorded in the v1.1 backlog; client-side filtering is acceptable for v1 given bundle sizes.

### AIFactory module layout

```
apps/backend/integrations/skill_pool/
├── __init__.py
├── client.py          # async httpx client; ~100 LOC; retries, timeouts, healthz
├── resolver.py        # git remote → project → bundle; honors override pin; ~150 LOC
├── cache.py           # content-addressed (sha256) blob store; ~80 LOC
├── models.py          # ResolvedBundle, PinnedSkill, SkillPoolError dataclasses
├── README.md          # how this works, troubleshooting
└── tests/
    ├── test_client.py
    ├── test_resolver.py
    ├── test_cache.py
    ├── test_injector.py
    └── test_contracts.py     # snapshot tests vs pinned skill_pool response fixtures

apps/backend/prompts_pkg/
└── skill_pool_injector.py    # inject(prompt, phase, ctx) → prompt'
                               # parses frontmatter, filters phase, idempotent
```

### Settings model — DB delta

**Important context.** The existing `apps/web-server/server/routes/projects.py` is **JSON-file-backed** (`load_projects()` / `save_projects()` against `projects.json`, called ~15 times in that file). No route currently touches the SQLAlchemy `Project` ORM model. To avoid entangling this integration with a JSON→ORM migration of the project surface, we use a **dedicated ORM-only sidecar table** that joins back to `project_id`. The existing JSON-backed project flow stays unchanged; only the new skill_pool config endpoint reads/writes the new table.

```python
# apps/web-server/server/database/models.py — new sidecar ORM table
class SkillPoolProjectConfig(Base):
    __tablename__ = "skill_pool_project_configs"

    project_id:           Mapped[str] = mapped_column(primary_key=True)
    # ^ String key that matches the project id used in projects.json.
    #   Intentionally NOT a SQL ForeignKey — `projects` is not an ORM-managed
    #   table today. Implementer note: do not add ForeignKey("projects.id") here.
    skill_pool_endpoint:  Mapped[str | None] = mapped_column(nullable=True)
    skill_pool_token:     Mapped[str | None] = mapped_column(_EncryptedString(), nullable=True)
    skill_pool_pin:       Mapped[str | None] = mapped_column(nullable=True)
    updated_at:           Mapped[datetime] = mapped_column(default=lambda: datetime.now(UTC))
```

One row per configured project; absence of a row = `UNCONFIGURED` state. Alembic migration follows the existing filename convention: `<YYYYMMDD>_<rev>_add_skill_pool_project_configs.py` (e.g. `20260601_a1b2c3d4e5f6_add_skill_pool_project_configs.py`).

**Encryption — verified reuse target.** The `skill_pool_token` column uses the established `EncryptedString` pattern (`apps/web-server/server/crypto/encrypted_string.py`), the same way `EmailAccount.access_token` (line 497), `EmailAccount.refresh_token` (line 501), `LlmEndpoint.api_key` (line 545), and `ApiKey.token` (line 455) are encrypted today. Single column, no `_ct`/`_dek_id` split — DEK rotation walks the org-scoped `kms_data_keys` table, not per-column DEK pointers. Provisioned by the P2.3 `encrypt_credentials` migration; covered by `crypto/rotation.py` automatically by virtue of using `EncryptedString`.

**API endpoint.** `PUT /projects/{projectId}/skill-pool-config` (matches the existing `{projectId}` path-param convention in `routes/projects.py:30-36`). The handler is the **first ORM-backed write to this file** — call it out in the PR description so reviewers know it's intentional. Reads use `select(SkillPoolProjectConfig).where(...)`, writes use upsert. Two follow-up routes for completeness: `GET /projects/{projectId}/skill-pool-config` (decrypts and returns the config with token masked) and `DELETE /projects/{projectId}/skill-pool-config` (clears the config, returns project to UNCONFIGURED).

### Cache layout

Lives under the workspace root (laptop default `~/.aifactory/workspaces/<workspace_id>/.skill_pool_cache/`, K8s default `<PVC>/<workspace_id>/.skill_pool_cache/`). Per-workspace, not per-task — tasks in the same workspace share the cache safely because reads are content-addressed.

```
.skill_pool_cache/
├── manifest.json                       # ResolvedBundle: pinned versions + sha256s + fetched_at
└── blobs/
    └── <sha256-hex>.md                 # one file per skill-md; immutable, deduped
```

Cache key includes `git-remote-url + plan-version` so it correctly invalidates when the curator publishes a new plan version. Survives worktree wipes. Manifest write uses a file lock to handle concurrent task starts.

### Phase tag — frontmatter convention

```yaml
---
name: acme-axum-handler-style
version: 1.4.0
kind: skill
phase: coder              # planner | coder | qa | all (default: all)
tags: [rust, axum]
---
```

Single string field, free-form (skill_pool's tag system already round-trips arbitrary frontmatter). AIFactory's injector reads `frontmatter.get("phase", "all")` and filters per call. Invalid phase values (anything outside the four allowed) cause the skill to be skipped + a per-skill audit row.

### Injector contract

```python
def inject_skill_pool(
    base_prompt: str,
    phase: Literal["planner", "coder", "qa"],
    project_dir: Path,
    spec_record: SpecRecord,            # for first-run pinning
) -> str:
    """Returns base_prompt with a fenced skill block prepended.

    Soft-fails (returns base_prompt unchanged + emits audit event) on:
      - skill_pool unreachable (first run only — cache covers retries)
      - no project for this git remote (404)
      - skill fetch returns non-200

    Hard-fails on: malformed frontmatter (programming error in the skill,
                   not infrastructure).

    Idempotent: tags the injected block with `<!-- skill_pool:v1 -->`
    and refuses to re-inject if marker present.
    """
```

## Data flow

### Sequence A — first run, happy path

```
1. User clicks Run on an AIFactory task (or /handover triggers)
2. AIFactory clones the repo into a worktree
3. backend.tasks.start_task() → resolver.resolve(worktree, project)
4. resolver:
   a. git remote get-url origin
   b. decrypt config.skill_pool_token (EncryptedString column — transparently
      decrypted on row load via SQLAlchemy type adapter)
   c. project.skill_pool_pin set? → use it directly (skip a/b)
      else → GET /v1/projects/resolve?remote=<url>
   d. GET /v1/tenant/projects/<slug>/plan → [{skill_slug, kind, version, tags}]
   e. Parallel (8-way) → GET /v1/skills/<slug>/skill-md?version=<v>&kind=<k>
      with 5s connect / 30s read timeout, 1 retry on 5xx
   f. Write blobs/<sha256>.md + manifest.json
   g. Record ResolvedBundle into spec_record
5. Each phase: prompt_generator → inject_skill_pool(...)
6. injector parses frontmatter, filters by phase, returns prompt + fenced block
7. Phase prompt dispatched to configured LLM provider
```

Typical wall-clock cost for first run: 150–400 ms (5–20 skills, parallel fetches).

### Sequence B — re-run (cache hit)

```
1–3. Same as A
4.   resolver reads manifest.json + verifies blobs exist → returns ResolvedBundle in <10 ms
     No network calls. No token decryption (no calls to make).
5–7. Same as A
```

Path every retry, re-run, and overnight `/handover` continuation takes.

### Sequence C — soft-fail on first run

```
1–3. Same as A
4a-b. Same as A
4c. GET /v1/projects/resolve?remote=… → connection refused / 503 / timeout
5.  resolver catches SkillPoolError; logs to:
      - task_logger (Live Agent Console)
      - audit_log (hash-chained, AIFactory existing pattern)
      - spec_record (skill_pool_status: "unreachable")
6.  resolver returns ResolvedBundle.empty()
7.  injector returns base_prompt unchanged
8.  AIFactory runs with bare-default prompts
```

Developer sees a yellow chip in the Live Console: *"Ran without skill_pool injection — see audit."* No silent degradation; no halt.

### Sequence D — 404 (no project for repo)

```
4c. GET /v1/projects/resolve?remote=… → 404
5.  resolver records spec_record.skill_pool_status: "no_project_for_repo"
    Emits a developer-facing hint in the console:
      "No skill_pool project for git@github.com:acme/billing.git.
       Run `skill-pool project init` in that repo or set the override pin
       in AIFactory project settings."
6.  ResolvedBundle.empty(). Same downstream as Sequence C.
```

### State machine

```
[UNCONFIGURED] --paste token--> [CONFIGURED, EMPTY_CACHE]
                                       |
                                       | first task run, skill_pool reachable
                                       v
                              [PINNED, CACHE_WARM] <----. (every subsequent run)
                                       |                |
                                       | cache miss     |
                                       | (curator       |
                                       |  bumped pin)   |
                                       v                |
                              [REFETCH, partial cache]--'

[CONFIGURED, EMPTY_CACHE] --first run, skill_pool down--> [DEGRADED]
                                                              | manual reset OR
                                                              | next run pulls successfully
                                                              v
                                                       [PINNED, CACHE_WARM]
```

## Error handling + edge cases

| # | Failure | Severity | Developer-facing | Audit / log |
|---|---|---|---|---|
| 1 | skill_pool unreachable on first run | Soft | Yellow chip: "Ran without skill_pool injection" | `skill_pool_status: unreachable` |
| 2 | 404 — no project for git remote | Soft | Hint: run `skill-pool project init` or set override | `skill_pool_status: no_project_for_repo` |
| 3 | 401/403 — token revoked / wrong scope | Soft + sticky | Settings banner: "skill_pool token needs attention" until reset | `project.skill_pool_health: auth_failed` |
| 4 | 200 with empty plan | None | (Silent — no skills curated yet is a valid state) | task_log: `skill_pool_skills_injected: 0` |
| 5 | Pinned skill version no longer exists | Partial | "3 of 12 skills could not be resolved" | per-skill audit |
| 6 | Single skill-md returns 4xx | Partial | Same as #5 | per-skill audit |
| 7 | Single skill-md returns 5xx | Partial + 1 retry | Same as #5 if retry fails | per-skill audit + ops alert on pattern |
| 8 | Malformed YAML frontmatter | Hard (curator bug) | Task aborts: "Skill `<slug>@<v>` has malformed YAML — fix in skill_pool curator." | audit + curator notification |
| 9 | KMS decrypt fails | Hard | Same path as git-PAT decrypt failure | reuse existing secret-store error |
| 10 | Cache corruption (manifest, no blobs) | Refetch | Silent unless refetch also fails (then Soft) | task_log info |
| 11 | Disk full when writing cache | Hard (worker resource) | Existing workspace-health check fires first | reuse |
| 12 | Override pin references nonexistent project / version | Hard | Settings UI: error on save. First run: "Override pin `acme/billing@2.0.0` not found." | audit |
| 13 | `phase` tag has invalid value | Partial (skip skill + warn) | "1 skill has invalid phase tag — see audit" | per-skill audit |
| 14 | Two skills resolve to same `name` | Hard (data integrity) | "skill_pool data integrity issue — file a bug" | audit + alert |
| 15 | Bundle exceeds size cap (50 total / 20 per phase) | Truncate by tag relevance | "Bundle truncated to 20 skills for coder phase" | audit + `skill_pool_truncated` metric |
| 16 | Concurrent task runs in same workspace | n/a (cache is per-workspace, content-addressed, read-safe) | n/a | n/a |
| 17 | Cache present, skill_pool has newer versions | Use cache (pinned) + show upgrade hint | Settings link: "Reset cache to upgrade" | task_log info |

### Size cap rationale

Average AIFactory phase prompt today: 3–8 KB. Average `SKILL.md`: 1–4 KB. 20 skills × 3 KB = 60 KB ≈ 15 K tokens. Meaningful but not catastrophic for the planner phase budget. Above 20 per phase, context bloat hurts coder phase quality more than skill curation helps. **Configurable per skill_pool tenant via `tenant.aifactory_max_skills_per_phase`; default 20.**

### Trust boundary

Skill content is treated as **untrusted prompt material** by AIFactory. A malicious skill saying *"ignore all prior instructions"* is exactly the prompt-injection threat the existing security model addresses:

- skill_pool's RBAC controls who can publish skills to a tenant.
- skill_pool's secret-scan rejects bundles shipping credentials.
- AIFactory's audit log captures which skills were injected into which task.

**This design does not sanitize, lint, or rewrite skill content.** AIFactory injects what skill_pool returns. The trust boundary is the publish step, not the read step.

## Testing strategy

### 1. Unit (pytest) — `apps/backend/integrations/skill_pool/tests/`

- `test_resolver.py` — git-remote parsing variants; override pin precedence; pin format accept/reject.
- `test_cache.py` — manifest round-trip; blob dedup; missing-blob refetch trigger; file lock under concurrent writers.
- `test_client.py` — 401/403/404/5xx → exception subclasses; timeout honored; 8-way parallel; one retry on 5xx.
- `test_injector.py` — missing `phase` defaults to `all`; invalid phase → skip + audit; idempotency marker prevents re-injection; size cap truncates by tag relevance.

Target >90% on the new package. No network, fast, runs every PR.

### 2. Contract tests — `tests/test_contracts.py`

Pinned JSON snapshots under `tests/fixtures/skill_pool/`:

```
projects-by-git-remote.200.json
project-plan.200.json
skill-md.200.txt
healthz.200.json
project-by-git-remote.404.json
```

If skill_pool changes a field name or type, AIFactory CI breaks **before** deployment. Refreshed via `make refresh-skill-pool-fixtures` running against a real skill_pool.

### 3. End-to-end happy path — `tests/e2e/test_skill_pool_integration.py`

Spins up `server/compose.yaml` from skill_pool (Postgres + MinIO + Caddy + Rust server). Seeds 5 phase-tagged skills + 1 project. Asserts:

- Injected skills appear under fenced block
- Phase filter honored (no leak)
- Size cap respected (seed 25 coder skills, assert 20 inject)
- Spec record captures pinned versions
- Cache dir contains exactly the expected blobs

Wall-clock budget: < 30 s per CI run. `@pytest.mark.e2e`, integration job.

### 4. Soft-fail — `tests/e2e/test_skill_pool_failures.py`

- Kill skill_pool container mid-run → assert Soft path + `skill_pool_status: unreachable`.
- Unregistered git remote → assert no_project_for_repo path.
- Mock client returns malformed YAML → assert Hard fail with curator-actionable message.

### 5. Performance smoke — `tests/perf/test_skill_pool_latency.py`

Nightly only:

- 50-skill bundle, first run, parallel fetch: < 1 s wall-clock with 50ms mocked per-fetch latency.
- 50-skill bundle, cache hit: < 50 ms.
- O(1) cache reads — no full-bundle re-parse per phase call.

### 6. skill_pool side

`server/tests/aifactory_phase_tag.rs` — round-trip a `phase: coder` skill through publish + fetch, verify the field survives. Cheap regression guard.

### CI integration

- AIFactory existing pipeline: unit + contract on every PR (+10 s).
- AIFactory integration job (existing): E2E + soft-fail on every PR (+90 s).
- Nightly perf lane: performance smoke once a day.
- skill_pool CI: new test in the standard Rust suite (+1 s).

### Deliberately not tested

- No new Playwright pyramid for Settings UI — one new spec in AIFactory's existing config.
- No load tests — 50-skill cap; throughput isn't the bottleneck.
- No prompt-quality A/B — curation question, not integration-correctness question.

## Migration / rollout

This is a feature-add to AIFactory + a documentation-add to skill_pool. The only schema change is one Alembic migration creating the new `skill_pool_project_configs` sidecar table — additive, no data migration required. **All existing AIFactory projects work unchanged** — they enter the `UNCONFIGURED` state on the state machine and behave exactly as today (no config row = no integration = bare-default prompts).

**Rollout sequence.**
1. Land the skill_pool-side docs + the round-trip test (`docs/wiki/Phase-Tag-Convention.md`, `docs/integrations/aifactory.md`, `server/tests/aifactory_phase_tag.rs`). Zero risk; pure additions.
2. Land the AIFactory backend changes behind a feature flag (`SKILL_POOL_INTEGRATION_ENABLED=true`). Default off in production; on in dev + staging.
3. Land the AIFactory frontend Settings section behind the same flag.
4. Internal dogfood: configure the integration on AIFactory's own development project (which uses the skill_pool repo). Run for two weeks; collect any soft-fail / partial events.
5. Enable flag in production. Existing projects stay unconfigured; opt-in per project.
6. Deprecate flag at next minor.

**Backwards compatibility.** None to break. The columns are additive; the prompt generator gracefully no-ops when the integration isn't configured.

## Open questions / follow-ups (not blocking)

1. **Should the size cap default differ between phases?** Planner can tolerate more context than coder (planner is reasoning-heavy; coder needs sharp focus). Current design: same cap. Worth measuring.
2. **Per-developer skill_pool API key down the line.** AIFactory's May 2026 `acw_` scope-gated keys could pair with a future per-developer token in skill_pool. Out of scope for v1.
3. **Should the `phase` tag also gate skill_pool's `direnv allow` install for humans?** Today direnv installs all matching skills; if AIFactory respects `phase`, should the human path too? Probably no — humans are general-purpose; skip.
4. **Pinning policy when curator force-updates a published version.** skill_pool's invariant is that publish bumps the version. If a curator ever yanks-and-republishes at the same version (recovery scenario), cached AIFactory tasks won't notice. Acceptable for v1.
5. **Convenience endpoint `GET /v1/projects/{slug}/bundle?phase=<p>`.** Server-side filtering. Saves a round-trip and a parse but adds a new endpoint to maintain. Decide in v1.1 based on bundle-size telemetry.

## Decision log

| Date | Decision | Rationale | Reversible? |
|---|---|---|---|
| 2026-05-29 | Direction: skill_pool → AIFactory | Highest immediate leverage; smallest new surface | Yes — capture direction is its own deliverable |
| 2026-05-29 | Mechanism: prompt-time injection | Works across all AIFactory providers; existing hook point | Yes — could switch to MCP-time later |
| 2026-05-29 | Resolution: auto-discover + override pin | Matches skill_pool's existing bootstrap; escape hatch for edge cases | Yes — could move to manifest-file-only |
| 2026-05-29 | Phase routing: frontmatter tag | Zero schema change; convention over coupling | Yes — could replace with first-class field |
| 2026-05-29 | Auth: per-project KMS-encrypted token | Survives `/handover`; matches existing PAT pattern | Yes — OIDC s2s is the next-gen path |
| 2026-05-29 | Reliability: cache + pin + soft-fail first run | Aligns with AIFactory spec-first ethos; no halt | Partially — pinning is hard to undo without spec migration |
| 2026-05-29 | v1 scope: all three phases | Plumbing is the same; phase fan-out is ~30 LOC | n/a |
| 2026-05-29 | Spec location: `skill_pool/docs/plans/` | super-brainstorm convention; AIFactory link added at implementation time | n/a |
