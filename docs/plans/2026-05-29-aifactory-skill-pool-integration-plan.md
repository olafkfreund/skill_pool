# AIFactory ↔ skill_pool integration — implementation plan

> **Pairs with:** [`2026-05-29-aifactory-skill-pool-integration-design.md`](2026-05-29-aifactory-skill-pool-integration-design.md)
> **Created:** 2026-05-29
> **Status:** Ready for execution

---

## Plan shape

Six PRs across two repos. Sequenced for lowest blast radius first; PRs 1–2 are pure additions that can land any time. PRs 3–5 land in AIFactory; PR 6 is the dogfood + flag flip.

```
skill_pool      AIFactory                       Risk     Effort
─────────────   ──────────────────────────────  ──────  ──────
PR 1: docs+test                                  none    XS  (½ day)
                PR 2: skill_pool client+resolver+cache+models  low     M   (3 days)
                PR 3: skill_pool_injector + 3 hook wraps       low     S   (1 day)
                PR 4: SkillPoolProjectConfig ORM + migration   low     S   (1 day)
                PR 5: frontend Settings → Integrations         low     M   (2 days)
                PR 6: dogfood + flag flip                       med     XS  (½ day)

Total: ~8 dev-days. Feature-flagged until PR 6.
```

Hidden behind `SKILL_POOL_INTEGRATION_ENABLED=true` env var until PR 6 — backend wires the flag in PR 2.

---

## PR 1 — skill_pool side: docs + phase-tag round-trip test

**Repo:** skill_pool ([`/home/olafkfreund/Source/GitHub/skill_pool/`](/home/olafkfreund/Source/GitHub/skill_pool/))
**Branch:** `feat/aifactory-integration-docs`
**Effort:** ½ day · **Risk:** none (pure additions)

### Changes
- `docs/wiki/Phase-Tag-Convention.md` — new. Documents `phase: planner|coder|qa|all` frontmatter tag, default behavior, validation rules.
- `docs/integrations/aifactory.md` — new. Operator guide: issuing the AIFactory token, scoping it `read`, curating phase-tagged skills.
- `server/tests/aifactory_phase_tag.rs` — new integration test. Round-trips a skill with `phase: coder` through publish + fetch; asserts frontmatter survives intact.
- `site/architecture.html` and `site/api.html` — add one paragraph each linking to the new docs (the GitHub Pages showcase site we just shipped).

### Verification
- `nix develop -c cargo test -p skill-pool-server aifactory_phase_tag`
- Manual: publish a skill with `phase: coder`, fetch via `GET /v1/skills/<slug>/skill-md`, confirm tag present.

### Why first
- No risk: pure additions to docs + one test. Approving it is a 5-minute review.
- Unblocks AIFactory side: PRs 2–6 reference these docs as the contract surface.
- Independently shippable. Doesn't need AIFactory to do anything.

---

## PR 2 — AIFactory side: skill_pool integration module (client + resolver + cache + models)

**Repo:** AIFactory ([`/tmp/AIFactory/`](/tmp/AIFactory/))
**Branch:** `feat/skill-pool-integration-module`
**Effort:** 3 days · **Risk:** low (new module, no edits to existing pipeline)

### Changes — new files only
```
apps/backend/integrations/skill_pool/
├── __init__.py          # public exports: ResolvedBundle, resolve, SkillPoolError
├── client.py            # async httpx; healthz, projects/resolve, plan, skill-md
├── resolver.py          # git remote → project → ResolvedBundle; honors pin
├── cache.py             # content-addressed manifest.json + blobs/<sha>.md
├── models.py            # ResolvedBundle, PinnedSkill, SkillPoolError dataclasses
└── tests/
    ├── conftest.py
    ├── test_client.py
    ├── test_resolver.py
    ├── test_cache.py
    └── test_contracts.py
└── tests/fixtures/
    ├── projects-by-git-remote.200.json
    ├── project-plan.200.json
    ├── skill-md.200.txt
    ├── healthz.200.json
    └── project-by-git-remote.404.json
```

### Acceptance criteria
- Unit-test coverage > 90% on the new package.
- Contract tests pass against the fixtures (pinned JSON snapshots from a real skill_pool CI run).
- `make refresh-skill-pool-fixtures` works (one-shot against docker-compose'd skill_pool).
- Feature flag `SKILL_POOL_INTEGRATION_ENABLED` is read but defaults `false`; the module is dead code unless flipped on.

### Dependencies
- None on PR 1 (we use the fixture snapshots, not the live API). But PR 1 is the documentation the implementer reads to know the contract.

### Verification
- `pytest apps/backend/integrations/skill_pool/tests/ -v`
- No new dependencies beyond what AIFactory already vendors (`httpx`, `pydantic`).

---

## PR 3 — AIFactory side: injector + three hook wraps

**Repo:** AIFactory
**Branch:** `feat/skill-pool-injector`
**Effort:** 1 day · **Risk:** low (additive wrap; falls back to base prompt under flag-off)

### Changes
- `apps/backend/prompts_pkg/skill_pool_injector.py` — new. `inject_skill_pool(base_prompt, phase, project_dir, spec_record) -> str`. Idempotent (uses `<!-- skill_pool:v1 -->` marker). Parses frontmatter, filters by phase tag, formats fenced block.
- `apps/backend/prompts_pkg/prompt_generator.py` — wrap the return of `generate_planner_prompt` (phase: `planner`) and `generate_subtask_prompt` (phase: `coder`).
- `apps/backend/prompts_pkg/prompts.py` — wrap the return of `get_qa_reviewer_prompt` (phase: `qa`).

Each wrap is one line that conditionally short-circuits when the feature flag is off:
```python
return inject_skill_pool(base_return_value, "planner", project_dir, spec_record)
# inside inject_skill_pool: if not flag_enabled or not configured → return base_return_value
```

### Acceptance criteria
- Unit tests in `apps/backend/integrations/skill_pool/tests/test_injector.py` from PR 2 now hit real wraps.
- New test: `test_hook_wraps.py` mocks the integration and asserts each phase's prompt contains the fenced block when flag on, base prompt when flag off.
- No change in behavior when flag is off (backwards-compat invariant).

### Dependencies
- PR 2 must merge first (uses `integrations.skill_pool`).

---

## PR 4 — AIFactory side: SkillPoolProjectConfig ORM + Alembic migration

**Repo:** AIFactory
**Branch:** `feat/skill-pool-project-config-orm`
**Effort:** 1 day · **Risk:** low (additive schema, sidecar table)

### Changes
- `apps/web-server/server/database/models.py` — add `SkillPoolProjectConfig` model (per spec § Settings model).
- `apps/web-server/server/database/alembic/versions/<YYYYMMDD>_<rev>_add_skill_pool_project_configs.py` — Alembic migration creating the new table.
- `apps/web-server/server/routes/projects.py` — three new endpoints: `PUT/GET/DELETE /projects/{projectId}/skill-pool-config`. **First ORM-backed write in this file** — call out in PR description.
- New service file: `apps/web-server/server/services/skill_pool_config.py` — thin wrapper around ORM operations (upsert, fetch, delete). Keeps routes thin.

### Acceptance criteria
- Migration runs cleanly forward and backward (downgrade is `op.drop_table("skill_pool_project_configs")`).
- New routes covered by API tests (matching the existing `routes/projects.py` test patterns).
- Token is `EncryptedString`-encrypted on write, masked on GET (returns `****`-style placeholder), recoverable internally via `data_key_manager`.

### Dependencies
- Independent of PRs 2–3 in terms of merge order, but the integration is non-functional without all three. Recommend landing this **immediately before PR 3** so PR 3's tests can verify end-to-end token-flow.

---

## PR 5 — AIFactory side: frontend Settings → Integrations

**Repo:** AIFactory
**Branch:** `feat/skill-pool-settings-ui`
**Effort:** 2 days · **Risk:** low (new UI section, gated by feature flag)

### Changes
- `apps/frontend-web/src/routes/settings/integrations/+page.svelte` (or React equivalent — match the existing settings page pattern). New "skill_pool" card with:
  - Endpoint URL field
  - Token paste field (masked once set)
  - Override pin field (placeholder: `acme/billing-service@2.0.0`)
  - "Test connection" button → calls `GET /v1/healthz` via backend proxy
  - Save / Disconnect actions
- Storybook story for the card (matches existing pattern).
- Playwright spec: `tests/e2e/skill_pool_settings.spec.ts` — exercises happy path + a soft-fail (mocked unreachable).

### Acceptance criteria
- Feature flag hides the card when off.
- Test-connection roundtrips a real `GET /v1/healthz` and shows status (green / red).
- Token field never leaks the stored value back to the page once saved (mask + "Replace" button).
- Override pin field validates the `tenant/slug@version` format client-side.

### Dependencies
- PR 4 (the routes the UI calls).

---

## PR 6 — Dogfood + flag flip

**Repo:** AIFactory
**Branch:** `chore/skill-pool-integration-enable`
**Effort:** ½ day · **Risk:** med (production behavior change)

### Changes
- Configure the integration on AIFactory's own internal development project pointed at the skill_pool tenant for `olafkfreund/skill_pool` repo.
- Run for 2 weeks; collect: `skill_pool_skills_injected` metric, `skill_pool_truncated` rate, soft-fail audit events, any partial-bundle warnings.
- After dogfood, flip `SKILL_POOL_INTEGRATION_ENABLED=true` in production (per-project opt-in still required).
- Update operator runbook with new metrics + alert thresholds.

### Acceptance criteria
- ≥10 successful AIFactory task runs against the dogfood project, with injection logged in spec record.
- Zero hard-fail incidents during dogfood window.
- Operator runbook updated.

---

## End-to-end test gates (CI)

Land alongside PR 3 (when both module + hooks exist):

- **E2E happy path** (`tests/e2e/test_skill_pool_integration.py`): docker-compose'd skill_pool + 5 seeded phase-tagged skills + 1 project. Asserts injection per phase, phase filter, size cap, spec pin, cache contents.
- **E2E soft-fail** (`tests/e2e/test_skill_pool_failures.py`): three scenarios (server killed mid-run, 404, malformed YAML).
- Both behind `@pytest.mark.e2e` so they run in AIFactory's existing integration job, not on every unit test run. +90s to that job; acceptable.

---

## Hands-off readiness checklist

After PR 6 lands and the flag is on in production:

- [ ] **skill_pool curator UX:** the `phase` tag is documented in the wiki + portal's "skill authoring" guide
- [ ] **AIFactory operator UX:** runbook covers the `skill_pool_*` metrics + how to diagnose a soft-fail event
- [ ] **Monitoring:** Grafana dashboard adds `skill_pool_skills_injected_total{phase, project}` and `skill_pool_truncated_total{phase}` panels
- [ ] **Cost telemetry:** count tokens consumed by injected skills per phase per project (for the inevitable "how much is this costing us" question)
- [ ] **Alerts:** page on `skill_pool_status: auth_failed` rate > 0 across any project, OR `skill_pool_unreachable` rate > 5% in any 1-hour window
- [ ] **Documentation cross-link:** the skill_pool showcase site (https://olafkfreund.github.io/skill_pool/) gets a "Used by AIFactory" section pointing at the integration

---

## Out of scope (followup epics — not in this plan)

These are the deferred deliverables called out in the spec's "Out of scope" section. Each is its own future plan:

1. **Factory → Registry capture.** Post AIFactory transcripts to `/v1/drafts`. Estimated 1-2 weeks.
2. **MCP peer-bridge.** Cross-expose tools. Estimated 1 week.
3. **AIFactory packaged as a skill_pool plugin.** Distribution play. Estimated 3 days.
4. **OIDC service-to-service auth.** v2 of the auth model. Estimated 2 weeks (both sides).
5. **Semantic ranking** of skills to phase. Replace the `phase` tag convention with embedding-based routing. Estimated 1 week.

---

## Risk register

| Risk | Likelihood | Mitigation |
|---|---|---|
| Prompt bloat hurts coder phase quality | Med | 50/20 size cap; per-tenant tunable; metric tracks rate of truncation |
| skill_pool outage causes silent degradation | Low | Soft-fail emits loud audit + console chip; alerts on rate > 5% / hr |
| Token leakage via logs | Low | `EncryptedString` + log-redaction middleware on the new routes |
| Sidecar table drifts from projects.json (orphans) | Med | Periodic cleanup job: delete configs whose `project_id` no longer in projects.json. Scope for a v1.1 cron. |
| Curator publishes a malicious phase-tagged skill | Low | skill_pool RBAC + audit; documented trust boundary in spec |
| `routes/projects.py` JSON→ORM migration eventually lands and conflicts | Med | When it happens, the sidecar table becomes an FK target instead of a string-key match — no data loss, just a schema cleanup PR |

---

## Final commit message templates

PR 1: `docs(integrations): add aifactory integration docs + phase-tag convention`
PR 2: `feat(skill_pool): integrations.skill_pool module — client, resolver, cache (flagged)`
PR 3: `feat(skill_pool): wrap planner / coder / qa prompts with skill_pool injector`
PR 4: `feat(skill_pool): SkillPoolProjectConfig sidecar ORM + /projects/.../skill-pool-config routes`
PR 5: `feat(skill_pool): frontend settings → integrations card`
PR 6: `chore(skill_pool): enable integration flag in production after dogfood`
