-- Migration 0034: plugin mirror metadata columns
--
-- Issue #32: adds the columns needed by the plugin mirror background worker.
-- `upstream_url` was already added in 0031 (and carries a NOT NULL constraint
-- for sourcing_mode='mirror' via plugins_mirror_requires_url CHECK). The four
-- new columns below store the refresh schedule and the last-error audit trail
-- that the periodic sweep writes on every poll cycle.
--
-- Column semantics:
--   last_pulled_at      — wall-clock timestamp of the most recent successful
--                         clone/pull. NULL until the first mirror job succeeds.
--   pull_interval_secs  — refresh cadence in seconds, configurable per plugin.
--                         The sweep query treats NULL as "use the server default"
--                         (currently 86400 = 24 h). Minimum enforced by CHECK:
--                         300 s (5 min) matching the issue spec.
--   fetch_error         — human-readable error message from the last failed pull,
--                         NULL when the last pull succeeded (same pattern as
--                         tenant_project_plans.fetch_error from migration 0030).
--   fetch_error_at      — timestamp of the last failure; NULL on success or
--                         before the first pull attempt.
--
-- Forward-only project — manual rollback:
--   ALTER TABLE plugins
--     DROP COLUMN IF EXISTS last_pulled_at,
--     DROP COLUMN IF EXISTS pull_interval_secs,
--     DROP COLUMN IF EXISTS fetch_error,
--     DROP COLUMN IF EXISTS fetch_error_at;
--   DELETE FROM _sqlx_migrations WHERE version = 34;

ALTER TABLE plugins
  ADD COLUMN last_pulled_at     TIMESTAMPTZ,
  ADD COLUMN pull_interval_secs INT CHECK (pull_interval_secs IS NULL OR pull_interval_secs >= 300),
  ADD COLUMN fetch_error        TEXT,
  ADD COLUMN fetch_error_at     TIMESTAMPTZ;

-- Index to drive the sweep query: find mirror plugins whose refresh interval
-- has elapsed. Partial index on sourcing_mode='mirror' keeps it narrow —
-- internal and external plugins are never swept.
CREATE INDEX idx_plugins_mirror_due
  ON plugins(tenant_id, last_pulled_at, pull_interval_secs)
  WHERE sourcing_mode = 'mirror';
