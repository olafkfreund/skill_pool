# skill-pool — Project Review

> Reviewed 2026-05-20. Evidence-based assessment with multi-rubric scoring.

This document is the objective assessment requested at the end of the build session. It captures three independent research streams (internal audit, competitive landscape, engineering-quality benchmark) and synthesizes a final verdict. Numbers cited are from `git log`, `cargo test`, `wc -l`, and verified via `gh issue list` against `olafkfreund/skill_pool` as of the report timestamp.

---

## 1. Executive summary

skill-pool is a 3-day-old, AI-paced greenfield build of a self-hosted multi-tenant Claude Code skill/agent/command registry. In its 178 commits it has shipped all 5 phases of the original master plan, all 3 cross-cutting epics (multi-tenancy, theming, scaling), plus 2 features that were added mid-build (Projects, Plans). The artifact looks like a 6-12 month mid-stage OSS project on every visible dimension — test ratio, security controls, deploy breadth, doc depth — but it is 3 days old and unproven in production.

**The novel finding:** no 1:1 competitor exists. Tier-A players in the Claude Code ecosystem are either read-only catalogs (`anthropics/skills`), SaaS-only (Skill Creator), per-project-only (the built-in `.claude/skills/` Git share), or unmaintained community demos. Tier-B IDPs (Backstage, Cortex, Port) are too broad. Tier-C analogues (Verdaccio for npm, Hugging Face Hub for models) solve the same structural problem for different content. **A team adopting Claude Code at scale today has no purpose-built self-hosted registry to reach for.** skill-pool fills that gap.

**Final score: 52 / 60** across six engineering-quality rubrics. Two caveats: the artifact has not been pressure-tested by real users, real migrations, or real incidents; and the AI commit cadence is not a sustainable maintainer signal. Treat the score as "high-quality build, low-confidence in lived behaviour."

---

## 2. By the numbers

| Dimension | Count | Source |
|---|---|---|
| Project age | 3 days (first commit 2026-05-18, `da92563`) | `git log --reverse` |
| Total commits on `main` | 178 | `git rev-list --count HEAD` |
| Commits by day | 72 / 76 / 30 | `git log --pretty=%ad --date=short` |
| Source LOC | ~39,300 across server, cli, web | `wc -l` per crate |
| Test LOC | ~13,800 (45% test-to-source ratio) | `wc -l server/tests cli` |
| Doc LOC | ~14,900 across 59 markdown files | `wc -l docs/**/*.md` |
| Postgres migrations | 30 | `ls server/migrations/*.sql` |
| HTTP routes | 71 | `.route(` calls in `routes/mod.rs` |
| CLI subcommands | 21 top-level (init, login, ensure, add, search, publish, capture, project, plan, doctor, …) | clap enum in `cli/src/main.rs` |
| Server lib tests | 139 / 139 pass | `cargo test -p skill-pool-server --lib` |
| CLI tests | 158 / 158 pass | `cargo test -p skill-pool-cli` |
| Integration tests (testcontainers) | ~40 files | `ls server/tests/*.rs` |
| Web svelte-check | 4150 files / 0 errors / 0 warnings | `npm run check` |
| Clippy workspace `--all-targets -- -D warnings` | clean | most recent `cargo clippy` |
| `unsafe` blocks | 0 in non-test code | `grep -r 'unsafe' server/src cli/src` |
| GH issues open / closed | 3 / 9 | `gh issue list --state all` |
| GH PRs open / closed | 0 / 15 | `gh pr list --state all` |
| Deploy paths shipped | NixOS module, Docker Compose, Helm chart, Terraform AWS (EKS + RDS + ACM + ElastiCache + S3 + IAM-IRSA), GH Actions OIDC | `flake.nix`, `deploy/`, `.github/workflows/` |

---

## 3. What we built

### Phases against the original plan

| Phase | Status | Evidence |
|---|---|---|
| Phase 0 — install spike + test skill | partial | `scripts/install.sh` + `skills/test-skill/` shipped; manual `claude` session verification (issue #2) not yet run |
| Phase 1 — server + CLI MVP | shipped | issue #3 closed; 71 routes, 21 CLI subcommands |
| Phase 2 — Web UI | shipped | issue #4 closed; SvelteKit portal with catalog/editor/drafts/admin |
| Phase 3 — Auto-bootstrap | shipped | issue #5 closed; detection cache + 3-tier matching server-side |
| Phase 4 — Retrospective capture | shipped | issue #6 closed; Stop-hook scorer, SessionEnd queue, Haiku-Sonnet drafter, drafts inbox, desktop notification |
| Phase 5 — Lifecycle | shipped | issue #7 closed; embeddings, decay, dependency resolution, agents+commands, MCP transport, git mirror |
| #8 — Multi-tenancy + Claude Enterprise | shipped | issue #8 closed; OIDC/SAML/SCIM, audit, dedicated mode, MDM template, SIEM export |
| #9 — Theming + white-label | shipped | issue #9 closed; logo/palette/font/favicon/custom CSS, ACME custom domains, OG image generator |
| #10 — Scaling + ops | shipped | issue #10 closed; Redis queue+DLQ, OTLP, Prometheus, Helm + Terraform AWS, runbooks |
| Projects (added mid-build) | shipped | migration 0029, `tenant_projects` + `tenant_project_items`, bootstrap tier 0, CLI `project` subcommand, admin web page |
| Plans (added mid-build) | shipped | migration 0030, `tenant_project_plans` with versioning + auto-refresh, CLI `plan` subcommand, ensure-time sync to `.claude/PROJECT_PLAN.md`, read-only web view |

### Documentation

The wiki at `docs/wiki/` carries 17 pages (Home, Architecture, Multi-Tenancy, Tenant-Onboarding, CLI-Reference, API-Reference, Operator-Guide, Phase-4-Capture, Phase-5-Lifecycle, SSO-Setup, Custom-Domain-ACME, Theming, MCP-Integration, Bundled-Skills, FAQ, Decisions-Log, Projects) for a total of ~5,500 lines. Per-deploy operator guides live at `docs/deploy/{nixos,aws,kubernetes,single-node,github-actions}.md`. Operational runbooks for incident response, capacity planning, and rollback are at `docs/ops/`.

### Showcase assets

- VHS-rendered terminal demo (`docs/demo.webm`, 88s, 1.3 MB) covering onboarding, project discovery, and plan import
- 14 Playwright-captured portal screenshots (`docs/images/*.webp`) including the new Projects and Plans surfaces
- Demo seeder (`scripts/seed-demo.sh`) that imports 120 real skills + agents from `borghei/Claude-Skills` (MIT + Commons Clause attribution preserved) and seeds two demo projects with a live plan

---

## 4. Competitive position

### Tier A — Claude Code ecosystem (direct comparators)

| Tool | OSS? | Self-hosted? | Multi-tenant? | Capture from sessions? | Stars |
|---|---|---|---|---|---|
| `anthropics/skills` | yes (Apache 2.0) | n/a (read-only repo) | no | no | ~138k |
| Skill Creator (Anthropic) | no | no (Claude.ai plugin) | no | yes (eval mode) | n/a |
| Claude Code built-in `.claude/skills/` Git share | partial | yes (local) | no (per-project) | no | n/a |
| SkillHub | no | no (SaaS) | yes | no | n/a |
| claude-skill-registry (community) | yes | yes (Vercel demo) | no | no | ~200 |
| skillshare CLI (community) | yes | yes | no | no | ~100 |
| **skill-pool** | **yes (MIT)** | **yes** | **yes** | **yes** | new |

No Tier-A tool combines self-hosting + multi-tenancy + retrospective capture. The closest community projects are unmaintained or proof-of-concept scale. Anthropic's own skills repo is curated, not a registry.

### Tier B — Internal Developer Platforms (structural analogues)

| Tool | Solves | Relevant comparator? | Closest shared feature |
|---|---|---|---|
| Backstage + TechDocs | IDP + doc-as-code | partial | Software Catalog + docs |
| Cortex / OpsLevel / Port | Team automation, governance | weak | RBAC + multi-tenant |
| Verdaccio (private npm) | Package distribution, team sharing | strong | CLI install, version history, self-hosted |
| Hugging Face Hub (and KohakuHub self-hosted) | Model registry + discovery | strong | Multi-tenant, version control, team sharing |
| MCP Registry (Anthropic) | MCP server metadata | weak | metadata-only, not self-hostable by design |

**Verdict.** Verdaccio and the Hugging Face Hub are the closest structural analogues — both are self-hosted multi-tenant registries with version history. Neither targets AI skills. IDPs are overkill (assume K8s) and not skill-focused. A team that wanted today's feature set has four bad options: cobble `.claude/skills/` over Git, repurpose Verdaccio with custom packaging, stand up an IDP, or do nothing. skill-pool is the only purpose-built option.

---

## 5. Engineering quality, by rubric

Each area scored 0-10 with transparent reasoning. Sum is out of 60.

### 5.1 Test coverage and quality — 8 / 10
- 407 test functions across server + CLI + integration. 45% test-to-source LOC ratio is high by any benchmark.
- 0 `#[ignore]` markers; no flaky escape valves.
- CI runs `cargo test --workspace` against a real pgvector Postgres service container (`.github/workflows/ci.yml`).
- **Weakness:** zero `query_as!` / `query!` macros. All sqlx queries are runtime-checked, so the compile-time tenant-isolation gate documented in the original plan is absent. That is a real engineering shortcut.

### 5.2 Security posture — 8 / 10
- RBAC: 65 `require_admin` / scope guards across routes.
- Audit log: `audit::record_best_effort` writes to `audit_events` from every mutating admin path.
- Secret scanning on bundle publish: 4 custom regexes (AKIA, ghp_, gho_, PEM). Not gitleaks-grade.
- Per-tenant rate limiting: `rate_limit.rs` with `tenants.rate_limit_rpm`.
- Body-size cap: `RequestBodyLimitLayer` on the router.
- HTTPS enforcement on outbound plan fetches: present in `admin::fetch_url_as_markdown`.
- SQLi: all `format!`-style SQL constructions interpolate static `const` fragments only; user input always parameterized via `$N` bind. Manually verified across `members.rs`, `scim.rs`.
- Token storage: sha256-hashed (`auth.rs::hash_token`); plaintext token shown once at mint then dropped.
- Multi-tenant isolation: every business table has `tenant_id` FK to `tenants(id) ON DELETE CASCADE`.
- **Weaknesses:** no compile-time SQL safety (see 5.1), no SBOM or `cargo audit` in CI, no CSP header verification, no fuzzing.

### 5.3 Operational maturity — 9 / 10
- Five deploy paths shipped: NixOS module, Docker Compose, Helm chart, Terraform AWS (15 .tf files: EKS + RDS + ElastiCache + ACM + Route53 + S3 + IAM-IRSA + GH OIDC), GitHub Actions deploy workflow.
- Observability: OTLP exporter (`opentelemetry 0.31`) behind feature flag, Prometheus `/metrics`, structured tracing.
- Migrations auto-applied at boot via `sqlx::migrate!` AND explicit Helm pre-upgrade hook + break-glass workflow (`migrate.yml`). Both belt and suspenders.
- Graceful shutdown via `with_graceful_shutdown(shutdown_signal())` + SIGTERM handler.
- `/v1/healthz` for liveness; `/metrics` for readiness.
- Backup story documented across `docs/ops/` and `docs/deploy/`.
- **Weakness:** no docker-compose for local dev (rely on `nix develop`).

### 5.4 Documentation depth — 9 / 10
- 59 markdown files, ~14,900 total lines.
- Per-deploy-target operator guides (NixOS, AWS, Kubernetes, single-node, GH Actions).
- Operational runbook + rollback + capacity planning under `docs/ops/`.
- 15 enterprise-feature docs under `docs/enterprise/`.
- Wiki has a decisions log, full API reference, full CLI reference.
- README is 241 lines and now portfolio-grade after the showcase pass (no emojis per user preference; hero image + demo embed + screenshots + quickstart + features + architecture diagram).
- **Weakness:** no machine-readable OpenAPI spec checked into the repo — the API reference is hand-written markdown.

### 5.5 Code quality signals — 8 / 10
- Workspace `cargo clippy --all-targets -- -D warnings`: clean.
- svelte-check: 4150 files / 0 errors / 0 warnings.
- `unsafe` blocks: 0 in non-test code.
- `.unwrap()` density in `server/src`: 60 occurrences, ~90% in test modules or `Regex::new(...).unwrap()` at static-init time (acceptable). Production-path unwraps in the single digits.
- `.expect()` count: 30 (slightly higher than ideal — many are also at static init).
- 0 `#[ignore]` markers.
- **Weakness:** no `cargo deny` / `cargo audit` in CI; no Clippy pedantic profile.

### 5.6 Velocity — 10 / 10 (raw), 7 / 10 (sustainable)
- 178 commits across 3 calendar days (72, 76, 30).
- All 5 phases + 3 cross-cutting epics + 2 added features.
- Open PRs: zero. Dependabot bumps merged or reverted-with-tracking-issue.
- **This is AI-paced greenfield burst, not human maintainer cadence.** Score 10 for raw output; 7 if you weight sustainability.

### Final grade — 52 / 60

If the project were 6-12 months old, this would land near the top decile of mid-stage OSS Rust projects in this size range. Being 3 days old changes the meaning: every quality artifact is present, none of it is proven by real-world use.

---

## 6. Honest gaps

1. **Issue #2 — Phase 0 manual gate**. The user has not yet started a fresh `claude` session to confirm the test fixture installed by `scripts/install.sh` is actually discovered by Claude Code's loader. That is the only gate that proves end-to-end correctness from the user's perspective. Five minutes of human action; not done.
2. **Issue #27 — `get_project_plan` MCP tool**. Opened today. Plans are synced via the file path (`.claude/PROJECT_PLAN.md`) so Claude can already read them; the MCP tool would let Claude fetch them on demand. Polish, not blocker.
3. **Compile-time SQL safety**. No `query_as!` / `query!` macros. The compile-time tenant-isolation gate the original plan called out is absent. Any new query that forgets `WHERE tenant_id = $1` will compile and ship. Mitigated by integration tests but not eliminated.
4. **Web component tests**. Svelte-check covers types. There are no Vitest unit tests against Svelte components for the Projects/Plans editors. If a curator's interactive form breaks, the only signal is a real session catching it.
5. **No production incident data**. Everything looks good on paper. None of it has been pressure-tested by a real user under real load with real failures.
6. **AI-paced commit cadence**. 178 commits in 3 days is not a maintainer signal; it is the signature of a single concentrated effort. Adopting this project sets up a maintenance burden a human team has not yet validated they can carry.

---

## 7. Multi-rubric scoring

Four different ways to score the same project — pick the rubric that matters to you.

| Rubric | Score | Reasoning |
|---|---|---|
| Functional completeness vs. original plan | 95% | 5 of 5 phases + 3 of 3 cross-cutting + 2 bonus features. Phase 0 manual gate is the only outstanding mainline item. |
| Competitive coverage (features vs. any existing alternative) | 100% | No competitor combines self-hosting + multi-tenancy + retrospective capture + Projects + Plans. By definition, complete coverage of the gap. |
| Engineering quality (artifact view) | 87% | 52 / 60 across the six engineering rubrics above. Top-decile artifact quality. |
| Production-readiness (lived view) | 40% | Test ratios + security controls + deploy breadth all present, but zero real-world hours. No incidents, no migrations under load, no support history. The artifact is ready; the operating story is not. |

---

## 8. What this is worth, by context

- **Internal team registry for a 10-50 dev shop adopting Claude Code at scale**: high. The lifecycle features (decay, retrospective capture, version history, project-scoped bundles) directly address pain a real team will hit.
- **Enterprise platform for thousands of devs**: medium until proven. The multi-tenant + SSO + audit + Helm + Terraform AWS surface is right, but unstressed at scale. Need a 30-day pilot with at least one regulated tenant before counting on it.
- **Public OSS reference implementation for the Claude Code ecosystem**: high. There is no competitor in the same shape; this would be the canonical example of "how to host Claude Code skills for a team."
- **Production support contract**: low until a maintainer cadence is established. AI-paced greenfield bursts do not signal "this will get bug fixes for the next 24 months."

---

## 9. Recommended next steps

If you stop building today, the order to harden is:

1. **Run the Phase 0 manual gate (#2)** — 5 minutes. Either it proves the end-to-end path or it surfaces a real bug. Either outcome is more valuable than any further code.
2. **Pilot one real tenant** for 7 days against a non-production workload. Capture every Sentry / log error. The remaining 40% of the "lived view" score lives here.
3. **Wire `cargo audit` and `cargo deny` into CI**. Cheap defense, currently absent.
4. **Migrate to `sqlx::query!` / `query_as!`** for compile-time tenant-isolation enforcement. The largest open security shortcut.
5. **Finish issue #27** — the `get_project_plan` MCP tool. Small polish; closes the only deliberately deferred item from the Plans feature.

After those, the artifact ↔ lived view gap closes substantially.

---

## Appendix — research provenance

- Internal audit: `Explore` subagent reading commit history, migrations, route table, test counts, GH issue/PR state.
- Competitive landscape: `search-specialist` subagent surveying Anthropic skills ecosystem + Backstage-class IDPs + package-registry analogues. Citations to docs.claude.com, github.com/anthropics, backstage.spotify.com, verdaccio.org, huggingface.co.
- Engineering quality: general-purpose agent assessing test ratios, security controls, ops maturity, doc depth, code-quality signals, velocity.
- Synthesis (this document) cross-checked against `git log` and `gh issue list` directly to reconcile minor discrepancies between the agent reports (specifically: project age = 3 days, total commits = 178, confirmed via `git rev-list --count HEAD`).
