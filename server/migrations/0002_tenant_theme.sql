-- skill-pool 0002_tenant_theme
-- Per-tenant branding: colours, logo, brand name. Phase 2 / #9 scaffold.
-- Free + Team tiers use these columns. Enterprise white-label (custom CSS,
-- branded email templates) layers additional tables on top later.

CREATE TABLE tenant_theme (
    tenant_id    UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    brand_name   TEXT        NOT NULL,
    primary_      TEXT        NOT NULL DEFAULT '#2563eb',
    primary_fg   TEXT        NOT NULL DEFAULT '#ffffff',
    accent       TEXT        NOT NULL DEFAULT '#0ea5e9',
    bg           TEXT        NOT NULL DEFAULT '#ffffff',
    fg           TEXT        NOT NULL DEFAULT '#0f172a',
    muted        TEXT        NOT NULL DEFAULT '#f1f5f9',
    muted_fg     TEXT        NOT NULL DEFAULT '#64748b',
    border       TEXT        NOT NULL DEFAULT '#e2e8f0',
    radius       TEXT        NOT NULL DEFAULT '0.5rem',
    logo_uri     TEXT,
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TRIGGER tenant_theme_touch_updated_at
    BEFORE UPDATE ON tenant_theme
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

-- Validate hex colour strings (best-effort; the API also validates).
ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_hex_colours CHECK (
        primary_   ~ '^#[0-9A-Fa-f]{3,8}$' AND
        primary_fg ~ '^#[0-9A-Fa-f]{3,8}$' AND
        accent     ~ '^#[0-9A-Fa-f]{3,8}$' AND
        bg         ~ '^#[0-9A-Fa-f]{3,8}$' AND
        fg         ~ '^#[0-9A-Fa-f]{3,8}$' AND
        muted      ~ '^#[0-9A-Fa-f]{3,8}$' AND
        muted_fg   ~ '^#[0-9A-Fa-f]{3,8}$' AND
        border     ~ '^#[0-9A-Fa-f]{3,8}$'
    );
