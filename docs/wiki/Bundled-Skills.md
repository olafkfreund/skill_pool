# Bundled Skills

> The skill catalog that ships with this skill-pool instance. This
> page was generated from a live query against
> `GET /v1/skills?limit=200` for the `acme` reference tenant — your
> deployment may have more, fewer, or different entries depending on
> what your operator has published.

**Source:** [borghei/Claude-Skills](https://github.com/borghei/Claude-Skills)
([MIT + Commons Clause](https://github.com/borghei/Claude-Skills/blob/main/LICENSE)).
Browse the upstream repo to see source `SKILL.md` files; bundle
contents land under `<category>/<slug>/SKILL.md` (skills) or
`agents/<domain>/<slug>.md` (agents).

## Attribution

The bundled catalog is curated from the borghei/Claude-Skills
collection. Each skill is attributed to its upstream author in its
SKILL.md frontmatter (the `author` field is preserved on publish).

License posture: borghei/Claude-Skills ships under MIT + the Commons
Clause. You may use, modify, and self-host these skills inside your
organization. You may not "sell" the catalog as a SaaS to third
parties without separate arrangement with the upstream author. See
the upstream LICENSE for the exact text.

skill-pool itself (the registry server, CLI, web portal) is
separately licensed — see [README](https://github.com/olafkfreund/skill_pool#license)
for the project license.

## Total catalog size

**76 entries** in the `acme` reference catalog as of generation. The
upstream borghei catalog publishes ~120 entries; the difference is
the subset that this deployment has actually imported. Run
`skill-pool search` from your own CLI for the live count against
your tenant.

## How to install

```bash
# Single skill:
skill-pool add axum-handler

# Stack-based bootstrap (recommended):
cd ~/projects/my-app
skill-pool bootstrap
# detects rust/axum/postgres → recommends the matching subset
```

Or via MCP from inside Claude:

> **You:** install axum-handler from skill-pool

See [CLI Reference](CLI-Reference.md#add-add-agent-add-command) and
[MCP Integration](MCP-Integration.md).

## Catalog

| Slug | Version | Description |
|---|---|---|
| `a11y-audit` | 1.0.0 | This skill should be used when the user asks to "check accessibility", "audit WCAG compliance", "scan HTML for a11y issues", "check color contrast", or "find ac |
| `agent-designer` | 1.0.0 | Designs multi-agent system architectures with orchestration patterns, tool schemas, and performance evaluation. Use when building AI agent systems, designing ag |
| `agenthub` | 1.0.0 | Multi-agent DAG orchestration framework. Design, execute, and manage workflows where multiple AI agents collaborate on complex tasks with dependency graphs. Cov |
| `agent-protocol` | 1.0.0 | Design and implement AI agent communication protocols including MCP tool schemas, Google A2A protocol, OpenAI function calling, structured inter-agent messaging |
| `agent-workflow-designer` | 1.0.0 | Design and implement multi-agent orchestration systems with workflow DAGs, agent routing, handoff protocols, state management, and cost optimization. Use when b |
| `ai-security` | 1.0.0 | This skill should be used when the user asks to "scan AI systems for security threats", "check for prompt injection vulnerabilities", "assess model security pos |
| `analytics-engineer` | 1.0.0 | Expert analytics engineering covering data modeling, dbt development, data transformation, and semantic layer management. Use when building dbt models, designin |
| `api-design-reviewer` | 1.0.0 | Reviews REST API designs for quality, consistency, and breaking changes. Lints OpenAPI specs, generates API scorecards, and detects breaking changes between ver |
| `api-test-suite-builder` | 1.0.0 | Generate comprehensive API test suites from route definitions across frameworks. Covers auth testing, input validation, contract testing, load testing with k6,  |
| `aws-solution-architect` | 1.0.0 | Design AWS architectures for startups using serverless patterns and IaC templates. Use when asked to design serverless architecture, create CloudFormation templ |
| `axum-handler` | 1.0.0 | Pattern for axum tenant-scoped extractors |
| `browser-automation` | 1.0.0 | This skill should be used when the user asks to "build web automation scripts", "check browser automation for detection", "generate web scraping code", "create  |
| `business-intelligence` | 1.0.0 | Expert business intelligence covering dashboard design, data visualization, reporting automation, and executive insights delivery. Use when designing dashboards |
| `changelog-generator` | 1.0.0 | Generate changelogs and release notes from Conventional Commits. Covers commit parsing, semantic version bump detection, Keep a Changelog formatting, monorepo s |
| `ci-cd-pipeline-builder` | 1.0.0 | Design and generate CI/CD pipelines from detected project stack signals. Covers GitHub Actions, GitLab CI, CircleCI, and Buildkite with caching, matrix builds,  |
| `codebase-onboarding` | 1.0.0 | Analyze a codebase and generate comprehensive onboarding documentation including architecture overviews, key file maps, local setup guides, task runbooks, debug |
| `codex-cli-specialist` | 1.0.0 | This skill should be used when the user asks to "set up Codex CLI", "convert skills for Codex", "write cross-platform AI skills", "configure agents/openai.yaml" |
| `context-engine` | 1.0.0 | Context management engine for AI coding agents. Handles context window optimization, persistent memory across sessions, context retrieval strategies, token budg |
| `data-analyst` | 1.0.0 | Expert data analysis covering SQL querying, data visualization, statistical analysis, business reporting, and data storytelling. Use when writing SQL queries, b |
| `database-designer` | 1.0.0 | Provides expert-level database design with schema analysis, index optimization, and migration generation. Supports PostgreSQL, MySQL, MongoDB, and DynamoDB. Use |
| `database-schema-designer` | 1.0.0 | Design relational database schemas from requirements with normalization, migration planning, ERD generation, RLS policies, index strategies, and type generation |
| `data-scientist` | 1.0.0 | Expert data science covering machine learning, statistical modeling, experimentation, predictive analytics, and advanced analytics. Use when selecting ML algori |
| `dependency-auditor` | 1.0.0 | Scans project dependencies for vulnerabilities, license compliance issues, and upgrade opportunities across Python, Node.js, Go, and Rust. Use when auditing dep |
| `design-auditor` | 2.1.0 | Use when auditing UI/UX designs for quality, detecting AI-generated slop patterns, validating WCAG accessibility compliance, checking design system token adhere |
| `devops-workflow-engineer` | 1.1.0 | Use when designing GitHub Actions workflows, creating CI/CD pipelines, planning multi-environment deployments, optimizing pipeline cost and execution time, or i |
| `doc-drift-detector` | 2.0.0 | Detects documentation drift against code changes, scores staleness on a weighted 0-100 scale, validates API docs via AST parsing, and audits link integrity. Use |
| `docker-development` | 1.0.0 | This skill should be used when the user asks to "analyze a Dockerfile", "optimize Docker layers", "validate docker-compose", "check container best practices", o |
| `env-secrets-manager` | 1.0.0 | Complete environment and secrets management lifecycle. Covers .env file scaffolding, validation scripts, secret leak detection in git history, credential rotati |
| `focused-fix` | 1.0.0 | This skill should be used when the user asks to "fix a bug with minimal changes", "analyze change scope for a bugfix", "find the minimal set of files to change" |
| `google-workspace-cli` | 1.0.0 | This skill should be used when the user asks to "audit Google Workspace", "check GWS security settings", "set up Google Workspace authentication", "diagnose Wor |
| `helm-chart-builder` | 1.0.0 | This skill should be used when the user asks to "analyze Helm charts", "validate Helm values", "review chart structure", "check Kubernetes Helm templates", or " |
| `incident-commander` | 1.1.0 | Use when handling production incidents, classifying severity, reconstructing timelines, writing postmortems, generating communication templates, or building inc |
| `interview-system-designer` | 1.0.0 | Designs calibrated interview loops, competency-based question banks, and hiring calibration systems. Use when designing interview processes, creating hiring pip |
| `kafka-consumer` | 1.0.0 | Kafka consumer with backpressure and graceful shutdown |
| `llm-cost-optimizer` | 1.0.0 | This skill should be used when the user asks to "estimate LLM costs", "count tokens in prompts", "optimize prompt token usage", "compare model pricing", or "red |
| `mcp-server-builder` | 1.0.0 | Build production-ready MCP (Model Context Protocol) servers with tool definitions, resource providers, prompt templates, and transport configuration. Covers Ope |
| `migration-architect` | 1.0.0 | Plans zero-downtime migrations with compatibility validation, rollback strategies, and phased execution plans. Use when migrating databases, APIs, infrastructur |
| `ml-ops-engineer` | 1.0.0 | Expert MLOps engineering covering model deployment, ML pipelines, model monitoring, feature stores, and infrastructure automation. Use when deploying models to  |
| `monorepo-navigator` | 1.0.0 | Navigate, manage, and optimize monorepos with Turborepo, Nx, pnpm workspaces, and Changesets. Covers cross-package impact analysis, selective builds, dependency |
| `ms365-tenant-manager` | 1.0.0 | Microsoft 365 tenant administration for Global Administrators. Automate M365 tenant setup, Office 365 admin tasks, Azure AD user management, Exchange Online con |
| `observability-designer` | 1.0.0 | Designs comprehensive observability strategies including SLI/SLO frameworks, alerting optimization, and dashboard generation. Use when implementing monitoring f |
| `performance-profiler` | 1.0.0 | Systematic performance profiling for Node.js, Python, and Go applications. Covers CPU flamegraphs, memory leak detection, bundle analysis, database query optimi |
| `playwright-pro` | 2.0.0 | Production-grade end-to-end testing with Playwright. Covers test generation from user stories, page object patterns, locator strategy, flaky test diagnosis, Cyp |
| `prompt-engineer-toolkit` | 1.0.0 | Production prompt engineering frameworks for building, testing, versioning, and evaluating prompts. Covers chain-of-thought, few-shot design, system prompt arch |
| `prompt-governance` | 1.0.0 | This skill should be used when the user asks to "audit prompts for safety", "check prompts for injection vulnerabilities", "manage a prompt catalog", "version c |
| `pr-review-expert` | 1.0.0 | Systematic PR review with blast radius analysis, security scanning, breaking change detection, test coverage delta, and performance impact assessment. Produces  |
| `qa-browser-automation` | 2.1.0 | Use when performing browser-based QA testing, visual regression tracking, WCAG accessibility auditing, performance profiling, or health scoring web applications |
| `rag-architect` | 1.0.0 | Designs production-grade RAG pipelines with chunking optimization, retrieval evaluation, and pipeline architecture. Use when building a RAG system, selecting a  |
| `red-team` | 1.0.0 | This skill should be used when the user asks to "plan a red team engagement", "scope a penetration test", "design a security assessment methodology", "create ru |
| `release-manager` | 1.0.0 | Automates release management with changelog generation, semantic versioning, and release readiness checks. Use when preparing releases, generating changelogs, b |
| `release-orchestrator` | 2.1.0 | Use when running pre-release validation, generating changelogs, bumping semantic versions, scoring deployment readiness, or orchestrating end-to-end release pip |
| `runbook-generator` | 1.0.0 | Generate production-grade operational runbooks from codebase analysis. Covers deployment procedures, incident response, database maintenance, scaling operations |
| `saas-scaffolder` | 1.0.0 | Generate complete production-ready SaaS boilerplate with authentication, database schemas, billing integration (Stripe), multi-tenancy, API routes, dashboard UI |
| `secrets-vault-manager` | 1.0.0 | This skill should be used when the user asks to "generate Vault configurations", "plan secret rotation", "analyze vault audit logs", "manage secrets lifecycle", |
| `self-improving-agent` | 2.0.0 | Patterns for building AI agents that learn from their own execution, detect failure modes, and improve autonomously. Covers feedback loops, performance regressi |
| `senior-backend` | 1.0.0 | This skill should be used when the user asks to "design REST APIs", "optimize database queries", "implement authentication", "build microservices", "review back |
| `senior-computer-vision` | 1.0.0 | Computer vision engineering skill for object detection, image segmentation, and visual AI systems. Covers CNN and Vision Transformer architectures, YOLO/Faster  |
| `senior-data-engineer` | 1.1.0 | Use when designing data architectures, building batch or streaming pipelines, implementing data quality frameworks, optimizing ETL/ELT performance, working with |
| `senior-devops` | 2.1.0 | Use when building CI/CD pipelines, containerizing applications, managing Kubernetes clusters, provisioning cloud infrastructure with Terraform, implementing dep |
| `senior-frontend` | 1.0.0 | Frontend development skill for React, Next.js, TypeScript, and Tailwind CSS applications. Use when building React components, optimizing Next.js performance, an |
| `senior-fullstack` | 1.0.0 | Fullstack development toolkit with project scaffolding for Next.js/FastAPI/MERN/Django stacks and code quality analysis. Use when scaffolding new projects, anal |
| `senior-ml-engineer` | 1.0.0 | ML engineering skill for productionizing models, building MLOps pipelines, and integrating LLMs. Covers model deployment, feature stores, drift monitoring, RAG  |
| `senior-prompt-engineer` | 1.0.0 | This skill should be used when the user asks to "optimize prompts", "design prompt templates", "evaluate LLM outputs", "build agentic systems", "implement RAG", |
| `senior-qa` | 1.0.0 | This skill should be used when the user asks to "generate tests", "write unit tests", "analyze test coverage", "scaffold E2E tests", "set up Playwright", "confi |
| `senior-secops` | 1.0.0 | Comprehensive SecOps skill for application security, vulnerability management, compliance, and secure development practices. Includes security scanning, vulnera |
| `senior-security` | 1.0.0 | Performs STRIDE threat modeling, DREAD risk scoring, secret detection, and secure architecture design. Use when conducting threat models, reviewing code for sec |
| `skill-security-auditor` | 1.0.0 | Security audit and vulnerability scanning for AI agent skills before installation. Detects prompt injection in SKILL.md files, dangerous code patterns (eval, ex |
| `skill-tester` | 1.0.0 | Validates and scores Claude Code skill packages for quality, completeness, and best practices compliance. Tests Python scripts, checks YAML frontmatter, and gen |
| `snowflake-development` | 1.0.0 | This skill should be used when the user asks to "optimize Snowflake queries", "analyze Snowflake SQL performance", "size Snowflake warehouses", "review Snowflak |
| `sql-database-assistant` | 1.0.0 | This skill should be used when the user asks to "optimize SQL queries", "explore database schemas", "generate migration SQL", "analyze query performance", or "d |
| `stripe-integration-expert` | 1.0.0 | Implement production-grade Stripe integrations for SaaS billing. Covers subscription lifecycle management, checkout sessions, plan upgrades/downgrades with pror |
| `tdd-guide` | 1.0.0 | Guides red-green-refactor TDD workflows with test generation, coverage gap analysis, and multi-framework support. Use when writing tests first, analyzing covera |
| `tech-debt-tracker` | 1.0.0 | Scans codebases for technical debt with AST parsing, prioritizes debt items by impact, and generates trend dashboards. Use when tracking tech debt across a code |
| `tech-stack-evaluator` | 1.0.0 | Technology stack evaluation and comparison with TCO analysis, security assessment, and ecosystem health scoring. Use when comparing frameworks, evaluating techn |
| `terraform-patterns` | 1.0.0 | This skill should be used when the user asks to "analyze Terraform modules", "scan IaC for security issues", "review Terraform configurations", "check infrastru |
| `threat-detection` | 1.0.0 | This skill should be used when the user asks to "analyze logs for threats", "detect suspicious activity", "scan for brute force attempts", "identify injection a |

## Category breakdown

The upstream borghei catalog organises entries by category. The 76
imported here span:

- **Senior personas** — `senior-backend`, `senior-frontend`,
  `senior-devops`, `senior-fullstack`, `senior-ml-engineer`,
  `senior-data-engineer`, `senior-qa`, `senior-prompt-engineer`,
  `senior-security`, `senior-secops`, `senior-computer-vision`. Each
  brings opinionated guidance for that role's typical task shape.
- **Stack-specific** — `axum-handler` (Rust), `kafka-consumer`
  (Java/Scala), `playwright-pro` (TS/JS), `terraform-patterns`
  (HCL), `helm-chart-builder` (k8s), `docker-development`,
  `snowflake-development`.
- **Quality + ops** — `a11y-audit`, `dependency-auditor`,
  `design-auditor`, `incident-commander`, `release-manager`,
  `release-orchestrator`, `runbook-generator`, `tech-debt-tracker`,
  `pr-review-expert`.
- **AI/ML** — `agent-designer`, `agenthub`, `agent-protocol`,
  `agent-workflow-designer`, `ai-security`, `llm-cost-optimizer`,
  `mcp-server-builder`, `ml-ops-engineer`, `prompt-engineer-toolkit`,
  `prompt-governance`, `rag-architect`, `self-improving-agent`,
  `senior-prompt-engineer`.
- **Security** — `ai-security`, `red-team`, `secrets-vault-manager`,
  `senior-secops`, `senior-security`, `skill-security-auditor`,
  `threat-detection`, `env-secrets-manager`.
- **Data** — `analytics-engineer`, `business-intelligence`,
  `database-designer`, `database-schema-designer`, `data-analyst`,
  `data-scientist`, `sql-database-assistant`, `senior-data-engineer`.
- **Catalog meta** — `skill-tester`, `skill-security-auditor`,
  `doc-drift-detector`.

## Source repository

Every bundled entry is sourced from
[borghei/Claude-Skills](https://github.com/borghei/Claude-Skills).
Direct links (browse upstream to see the unmodified SKILL.md):

- Skills folder: <https://github.com/borghei/Claude-Skills/tree/main/skills>
- Agents folder: <https://github.com/borghei/Claude-Skills/tree/main/agents>
- Commands folder: <https://github.com/borghei/Claude-Skills/tree/main/commands>

To re-import an updated upstream version, publish through the
standard `POST /v1/skills` flow (or the `skill-pool publish` CLI) —
the catalog handles versioning the same way as your team's internal
skills.

## Refreshing this page

This page was generated by querying:

```bash
TOKEN=$(skill-pool-server admin token-create --tenant <slug> \
  --name wiki-bundled-list --scope "skills:read tenant:admin")

curl -s -H "Authorization: Bearer $TOKEN" \
        -H "x-skill-pool-tenant: <slug>" \
   'https://<your-host>/v1/skills?limit=200' \
  | jq -r '.[] | "| `\(.slug)` | \(.version) | \(.description[0:160]) |"'
```

To refresh, rerun the query and paste the table back into this wiki
page. (Future work: a `skill-pool-server admin export-catalog
--markdown` subcommand that emits this page wholesale.)

## Where to read next

- [CLI Reference](CLI-Reference.md) — every `skill-pool` verb
- [Phase 5 — Lifecycle](Phase-5-Lifecycle.md) — how the catalog evolves
- [MCP Integration](MCP-Integration.md) — search the catalog from Claude
- [API Reference](API-Reference.md#skills-catalog) — the underlying endpoints

## Cross-links into the codebase

- `server/src/routes/skills.rs` — catalog endpoint implementation
- `scripts/` — (future home of the export command)
- `skills/` — the in-repo test fixture skill used for Phase 0 verification
