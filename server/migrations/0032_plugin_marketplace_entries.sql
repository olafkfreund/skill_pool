-- Migration 0032: plugin_marketplace_entries
--
-- Denormalised cache of what each tenant's /.claude-plugin/marketplace.json
-- should contain. One row per (tenant, plugin slug). Updated whenever a
-- plugin is published/archived/mirror-refreshed.
--
-- Design notes:
--   - One row per (tenant, slug) — the "latest published version" pointer.
--     marketplace.json is a flat list of plugins, not versions.
--   - source_url is the URL Claude Code's `git clone` will hit (either the
--     external URL or skill-pool's own /git/plugins/<slug>.git endpoint).
--   - entry_json is the exact JSON object we splice into marketplace.json's
--     `plugins` array — pre-rendered to avoid per-request assembly.
--   - tenant_id FK to tenants(id) cascades on tenant deletion. Kept even
--     though the composite FK below also cascades — a tenant with zero
--     plugins must still be able to delete cleanly, which the plugin-side
--     FK can't guarantee on its own.
--   - Defense-in-depth: cross-tenant (tenant_id, plugin_id) consistency is
--     enforced at the SCHEMA layer via a composite FK to
--     plugins(tenant_id, id) — see migration 0031's idx_plugins_tenant_id_pk
--     unique index that backs it. This rejects any INSERT whose tenant_id
--     does not match the referenced plugin's tenant. Belt-and-braces with
--     the API handler check that lands in #30.

CREATE TABLE plugin_marketplace_entries (
  tenant_id      UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  plugin_slug    CITEXT      NOT NULL,
  plugin_id      UUID        NOT NULL,
  version        TEXT        NOT NULL,
  source_url     TEXT        NOT NULL,
  entry_json     JSONB       NOT NULL,
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, plugin_slug),
  -- Defense-in-depth: tenant_id and plugin_id must point at the same
  -- plugin row. Backed by idx_plugins_tenant_id_pk in migration 0031.
  CONSTRAINT plugin_marketplace_entries_plugin_tenant_match
    FOREIGN KEY (tenant_id, plugin_id)
    REFERENCES plugins(tenant_id, id) ON DELETE CASCADE
);

CREATE INDEX idx_plugin_marketplace_tenant
  ON plugin_marketplace_entries(tenant_id, updated_at DESC);
