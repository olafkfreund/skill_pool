-- Migration 0030: tenant_project_plans + per-project auto-refresh config
--
-- A "project plan" is a markdown document imported from an external source
-- (Confluence, Notion, GitHub, local file). Each import creates an immutable
-- version row. One row per project is always in `status = 'active'` and serves
-- as the source of truth for developers.
--
-- Design notes:
--   - Versions are monotonically increasing per project (MAX+1 on insert).
--   - Dedup: if the new body_sha256 matches the active row, import is a no-op.
--   - Only one row may be 'active' per project at a time; the partial unique
--     index `idx_project_plans_active_one` enforces this at the DB level.
--   - Fetch failures persist `fetch_error` + `fetch_error_at` on the ACTIVE
--     row without creating a new version — developers always see a valid plan.
--   - plan_auto_refresh_interval_secs on tenant_projects opts a project into
--     periodic re-fetch. NULL = explicit-only.
--   - last_plan_refresh_at is updated after every refresh attempt (success or
--     failure) so the sweep query can compute "due" projects efficiently.

CREATE TABLE tenant_project_plans (
  id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  tenant_id        UUID        NOT NULL REFERENCES tenants(id)          ON DELETE CASCADE,
  project_id       UUID        NOT NULL REFERENCES tenant_projects(id)  ON DELETE CASCADE,
  version          INT         NOT NULL,                   -- monotonic per project
  body_md          TEXT        NOT NULL,
  body_sha256      TEXT        NOT NULL,                   -- SHA-256 hex of body_md
  source_type      TEXT        NOT NULL CHECK (source_type IN ('file', 'url')),
  source_url       TEXT,                                   -- original URL or file path
  source_etag      TEXT,                                   -- HTTP ETag from last fetch
  imported_by      UUID        REFERENCES users(id) ON DELETE SET NULL,
  imported_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  status           TEXT        NOT NULL DEFAULT 'active'
                               CHECK (status IN ('active', 'superseded', 'archived')),
  fetch_error      TEXT,       -- last refresh failure message (non-null = last fetch failed)
  fetch_error_at   TIMESTAMPTZ,
  UNIQUE (project_id, version)
);

-- Only ONE active row per project at any point in time.
CREATE UNIQUE INDEX idx_project_plans_active_one
  ON tenant_project_plans(project_id) WHERE status = 'active';

-- Fast lookup of all versions for a project (list + detail endpoints).
CREATE INDEX idx_project_plans_project_version
  ON tenant_project_plans(project_id, version DESC);

-- Per-project auto-refresh configuration (hybrid opt-in):
--   NULL  = explicit import only
--   N     = background task re-fetches every N seconds
ALTER TABLE tenant_projects
  ADD COLUMN plan_auto_refresh_interval_secs INT;

-- Tracks when the last refresh was attempted (success or failure).
-- Used by the sweep query to determine due projects.
ALTER TABLE tenant_projects
  ADD COLUMN last_plan_refresh_at TIMESTAMPTZ;
