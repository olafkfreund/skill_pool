-- Migration 0029: tenant_projects + tenant_project_items
--
-- Projects are a first-class primitive that let curators bundle a specific
-- set of skills/agents/commands for a named project (e.g. "Acme Billing
-- Service"). Bootstrap gives project items highest precedence (tier 0),
-- with existing stack-mapping tiers backfilling remaining slots.
--
-- Design notes:
--   - slug is CITEXT (case-insensitive) matching the tenants table pattern
--   - git_remote stores a normalized URL (SSH→HTTPS, trailing .git stripped)
--   - The partial unique index on (tenant_id, git_remote) allows NULL remotes
--     (multiple projects without a remote) while enforcing uniqueness when set
--   - tenant_project_items uses a composite PK so re-adding the same item is
--     detectable at the DB level; the route handler uses an atomic
--     DELETE+INSERT transaction for full replacement

CREATE TABLE tenant_projects (
  id           UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  tenant_id    UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  slug         CITEXT      NOT NULL,
  name         TEXT        NOT NULL,
  description  TEXT,
  git_remote   TEXT,
  stack_tags   TEXT[]      NOT NULL DEFAULT '{}',
  created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, slug)
);

CREATE INDEX idx_tenant_projects_tenant
  ON tenant_projects(tenant_id);

CREATE UNIQUE INDEX idx_tenant_projects_remote
  ON tenant_projects(tenant_id, git_remote)
  WHERE git_remote IS NOT NULL;

CREATE TABLE tenant_project_items (
  project_id  UUID NOT NULL REFERENCES tenant_projects(id) ON DELETE CASCADE,
  skill_slug  TEXT NOT NULL,
  kind        TEXT NOT NULL CHECK (kind IN ('skill', 'agent', 'command')),
  position    INT  NOT NULL DEFAULT 0,
  PRIMARY KEY (project_id, skill_slug, kind)
);

CREATE INDEX idx_tenant_project_items_project
  ON tenant_project_items(project_id);
