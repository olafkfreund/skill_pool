-- Migration 0031: plugins + plugin_contents
--
-- Plugins are a Layer 3 primitive that bundle skills/agents/commands
-- (and inline hooks/MCP/LSP blobs in `manifest`) for distribution through
-- the per-tenant marketplace.json + git endpoint.
--
-- Design notes:
--   - slug is CITEXT, matching tenants/tenant_projects.
--   - One plugin row per (tenant, slug, version); status flips drive
--     marketplace visibility without delete.
--   - `sourcing_mode` is TEXT + CHECK (not a pg enum) so 0033 can append
--     mirror metadata columns without an ALTER TYPE.
--   - `manifest` is JSONB (the canonical .claude-plugin/plugin.json body).
--   - plugin_contents references the catalog by composite (slug, kind, version)
--     scoped to the same tenant — NOT a FK to skills.id, because the
--     manifest pins by slug+version, and using the natural key keeps cross-
--     version content swaps explicit. CASCADE on plugins ensures contents
--     vanish with their parent; tenant CASCADE on plugins handles the rest.
--
-- Manual rollback (forward-only project — drop in reverse FK order):
--   DROP TABLE IF EXISTS plugin_marketplace_entries;  -- from 0032 first
--   DROP TABLE IF EXISTS plugin_contents;
--   DROP TABLE IF EXISTS plugins;
--   DELETE FROM _sqlx_migrations WHERE version IN (31, 32);

CREATE TABLE plugins (
  id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
  tenant_id       UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
  slug            CITEXT      NOT NULL,
  version         TEXT        NOT NULL,
  name            TEXT        NOT NULL,
  description     TEXT,
  manifest        JSONB       NOT NULL,
  status          TEXT        NOT NULL DEFAULT 'draft'
                              CHECK (status IN ('draft', 'published', 'archived')),
  sourcing_mode   TEXT        NOT NULL DEFAULT 'internal'
                              CHECK (sourcing_mode IN ('internal', 'external', 'mirror')),
  external_git_url TEXT,
  upstream_url    TEXT,
  created_by      UUID        REFERENCES users(id) ON DELETE SET NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
  UNIQUE (tenant_id, slug, version),
  -- Sourcing-mode invariants: paired URL columns must be populated for
  -- the modes that carry them, and at least one URL must exist for any
  -- non-internal mode.
  CONSTRAINT plugins_external_requires_url
    CHECK (sourcing_mode <> 'external' OR external_git_url IS NOT NULL),
  CONSTRAINT plugins_mirror_requires_url
    CHECK (sourcing_mode <> 'mirror'   OR upstream_url     IS NOT NULL)
);

CREATE INDEX idx_plugins_tenant_slug
  ON plugins(tenant_id, slug);
CREATE INDEX idx_plugins_tenant_status
  ON plugins(tenant_id, status);
CREATE INDEX idx_plugins_tenant_recent
  ON plugins(tenant_id, created_at DESC);

-- Backing index for the composite FK from plugin_marketplace_entries
-- (added in 0032). The FK targets (tenant_id, id); pg requires a unique
-- index on those columns for the constraint to be legal. id alone is
-- already unique (PK), so the composite is naturally unique too — this
-- index just satisfies the constraint requirement.
--
-- Cost: ~24 bytes/row; benefit: schema-layer rejection of cross-tenant
-- plugin_id references in marketplace entries (defense-in-depth — belt
-- alongside the API handler braces landing in #30).
CREATE UNIQUE INDEX idx_plugins_tenant_id_pk
  ON plugins(tenant_id, id);

CREATE TABLE plugin_contents (
  plugin_id        UUID NOT NULL REFERENCES plugins(id) ON DELETE CASCADE,
  content_slug     TEXT NOT NULL,
  content_kind     TEXT NOT NULL CHECK (content_kind IN ('skill', 'agent', 'command')),
  content_version  TEXT NOT NULL,
  position         INT  NOT NULL DEFAULT 0,
  PRIMARY KEY (plugin_id, content_slug, content_kind, content_version)
);

CREATE INDEX idx_plugin_contents_plugin
  ON plugin_contents(plugin_id);

-- Reverse-lookup: "which plugins use this skill version?" — used by the
-- API layer in #2 to block deletion of an in-use skill.
CREATE INDEX idx_plugin_contents_lookup
  ON plugin_contents(content_slug, content_kind, content_version);
