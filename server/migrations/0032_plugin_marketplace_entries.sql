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
--   - tenant_id CASCADE handles tenant deletion; plugin_id CASCADE handles
--     plugin row deletion (which is rare — archive flips status instead).
--   - Defense-in-depth: cross-tenant (tenant_id, plugin_id) consistency is
--     enforced at the API layer in #2; the FK on plugin_id alone permits
--     mismatch at the schema layer. See plugin_schema.rs test
--     `cross_tenant_plugin_id_in_marketplace_entry_is_api_layer_concern`.

CREATE TABLE plugin_marketplace_entries (
  tenant_id      UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  plugin_slug    CITEXT      NOT NULL,
  plugin_id      UUID        NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  version        TEXT        NOT NULL,
  source_url     TEXT        NOT NULL,
  entry_json     JSONB       NOT NULL,
  updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
  PRIMARY KEY (tenant_id, plugin_slug)
);

CREATE INDEX idx_plugin_marketplace_tenant
  ON plugin_marketplace_entries(tenant_id, updated_at DESC);
