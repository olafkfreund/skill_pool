-- Migration 0033: allow `kind='plugin'` in tenant_project_items
--
-- Issue #36 (bootstrap): a project may now pin a plugin slug as one of
-- its curated items. The bootstrap tier-0 expander resolves a plugin
-- row through `plugin_contents` and merges the bundled skills/agents/
-- commands into the install plan with provenance metadata.
--
-- The original CHECK in migration 0029 restricted kind to the three
-- atomic catalog kinds (skill/agent/command). Plugins live in a
-- different table (`plugins`, migration 0031) but they're addressed
-- by slug too, so storing the pin in the same column with kind='plugin'
-- avoids inventing a parallel `tenant_project_plugin_items` table that
-- would need its own ORDER-BY-position dance to interleave with the
-- atomic items.
--
-- Forward-only project: rollback is `ALTER TABLE … DROP CONSTRAINT …`
-- followed by re-adding the old CHECK; not scripted here.

ALTER TABLE tenant_project_items
  DROP CONSTRAINT tenant_project_items_kind_check;

ALTER TABLE tenant_project_items
  ADD CONSTRAINT tenant_project_items_kind_check
  CHECK (kind IN ('skill', 'agent', 'command', 'plugin'));
