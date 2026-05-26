-- Migration 0035: allow republishing a previously-archived plugin version.
--
-- Why: routes::plugins::archive (DELETE /v1/plugins/{slug}/versions/{version})
-- is a soft-delete — it flips status='archived' and keeps the row for
-- audit/history. With the original 0031 table-level UNIQUE constraint a
-- subsequent publish of the same (tenant, slug, version) tuple hits a
-- duplicate-key violation and the handler returns 409. That contradicts
-- the documented archive-then-republish flow used by callers (and the
-- idempotency contract exercised in
-- server/tests/plugin_git_idempotent.rs).
--
-- Fix: replace the unconditional uniqueness with a partial unique index
-- that only considers non-archived rows. The active set still enforces
-- a single (tenant, slug, version); archived rows are excluded so a
-- republish slots into the same key cleanly.
--
-- Manual rollback (forward-only project):
--   DROP INDEX IF EXISTS plugins_tenant_id_slug_version_active_key;
--   ALTER TABLE plugins
--     ADD CONSTRAINT plugins_tenant_id_slug_version_key
--     UNIQUE (tenant_id, slug, version);
--   DELETE FROM _sqlx_migrations WHERE version = 35;

ALTER TABLE plugins DROP CONSTRAINT plugins_tenant_id_slug_version_key;

CREATE UNIQUE INDEX plugins_tenant_id_slug_version_active_key
  ON plugins (tenant_id, slug, version)
  WHERE status <> 'archived';
