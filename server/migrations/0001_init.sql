-- skill-pool 0001_init
-- Multi-tenant foundation: tenants, users, tokens, skills, audit.
-- Every business-data table carries tenant_id. Phase 5 adds embeddings, deps, usage.

CREATE EXTENSION IF NOT EXISTS pgcrypto;
CREATE EXTENSION IF NOT EXISTS citext;

-- ----------------------------------------------------------------------------
-- Tenants
-- ----------------------------------------------------------------------------
CREATE TABLE tenants (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    slug          CITEXT      NOT NULL UNIQUE,
    name          TEXT        NOT NULL,
    plan_tier     TEXT        NOT NULL DEFAULT 'team'
                              CHECK (plan_tier IN ('team', 'business', 'enterprise')),
    status        TEXT        NOT NULL DEFAULT 'active'
                              CHECK (status IN ('active', 'suspended', 'deleted')),
    region        TEXT,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_tenants_status ON tenants(status) WHERE status = 'active';

-- ----------------------------------------------------------------------------
-- Users (cross-tenant identity; membership via tenant_users)
-- ----------------------------------------------------------------------------
CREATE TABLE users (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email            CITEXT      NOT NULL UNIQUE,
    external_idp_id  TEXT,
    display_name     TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ----------------------------------------------------------------------------
-- Tenant membership + role
-- ----------------------------------------------------------------------------
CREATE TABLE tenant_users (
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id     UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role        TEXT        NOT NULL DEFAULT 'viewer'
                            CHECK (role IN ('viewer', 'publisher', 'curator', 'admin')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);

CREATE INDEX idx_tenant_users_user ON tenant_users(user_id);

-- ----------------------------------------------------------------------------
-- API tokens (CLI + machine-to-machine). Stored as SHA-256 hex.
-- Scope is space-separated capabilities.
-- ----------------------------------------------------------------------------
CREATE TABLE tenant_api_tokens (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    hashed_token  TEXT        NOT NULL UNIQUE,
    name          TEXT        NOT NULL,
    scope         TEXT        NOT NULL DEFAULT 'read',
    created_by    UUID        REFERENCES users(id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at  TIMESTAMPTZ,
    revoked_at    TIMESTAMPTZ
);

CREATE INDEX idx_tokens_tenant ON tenant_api_tokens(tenant_id) WHERE revoked_at IS NULL;

-- ----------------------------------------------------------------------------
-- Skills (Phase 1 — no embeddings, no dependencies; Phase 5 adds them)
-- ----------------------------------------------------------------------------
CREATE TABLE skills (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    slug          TEXT        NOT NULL,
    version       TEXT        NOT NULL,
    description   TEXT        NOT NULL,
    when_to_use   TEXT,
    tags          TEXT[]      NOT NULL DEFAULT '{}',
    status        TEXT        NOT NULL DEFAULT 'published'
                              CHECK (status IN ('draft', 'published', 'archived')),
    bundle_uri    TEXT        NOT NULL,
    bundle_sha256 TEXT        NOT NULL,
    created_by    UUID        REFERENCES users(id) ON DELETE SET NULL,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (tenant_id, slug, version)
);

CREATE INDEX idx_skills_tenant_slug   ON skills(tenant_id, slug);
CREATE INDEX idx_skills_tenant_recent ON skills(tenant_id, created_at DESC);
CREATE INDEX idx_skills_tags          ON skills USING GIN (tags);

-- ----------------------------------------------------------------------------
-- Audit log — append-only. Every mutating endpoint writes here.
-- ----------------------------------------------------------------------------
CREATE TABLE audit_events (
    id          BIGSERIAL   PRIMARY KEY,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    actor_user  UUID        REFERENCES users(id) ON DELETE SET NULL,
    actor_token UUID        REFERENCES tenant_api_tokens(id) ON DELETE SET NULL,
    action      TEXT        NOT NULL,
    target_kind TEXT        NOT NULL,
    target_id   TEXT,
    metadata    JSONB       NOT NULL DEFAULT '{}'::jsonb,
    ip_addr     INET,
    user_agent  TEXT,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX idx_audit_tenant_ts ON audit_events(tenant_id, ts DESC);
CREATE INDEX idx_audit_action    ON audit_events(tenant_id, action, ts DESC);

-- Append-only enforcement: no UPDATE, no DELETE on audit_events.
REVOKE UPDATE, DELETE ON audit_events FROM PUBLIC;

-- ----------------------------------------------------------------------------
-- updated_at trigger
-- ----------------------------------------------------------------------------
CREATE OR REPLACE FUNCTION touch_updated_at() RETURNS trigger AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER tenants_touch_updated_at
    BEFORE UPDATE ON tenants
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

CREATE TRIGGER users_touch_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
