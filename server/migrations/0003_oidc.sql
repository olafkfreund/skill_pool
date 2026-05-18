-- skill-pool 0003_oidc
-- Per-tenant OIDC SP config + user sessions issued after OIDC sign-in.
-- SAML and SCIM (still under #8) layer additional tables in later migrations.

CREATE TABLE tenant_sso (
    tenant_id      UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    issuer_url     TEXT        NOT NULL,
    client_id      TEXT        NOT NULL,
    client_secret  TEXT        NOT NULL,
    default_role   TEXT        NOT NULL DEFAULT 'viewer'
                               CHECK (default_role IN ('viewer', 'publisher', 'curator', 'admin')),
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TRIGGER tenant_sso_touch_updated_at
    BEFORE UPDATE ON tenant_sso
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();

-- Users get a stable handle for "the same IdP subject identity". The same
-- email may sign in via two different IdPs across two tenants; we keep them
-- as one user row but with possibly different external_idp_id values
-- captured at first contact.
CREATE INDEX idx_users_external_idp_id ON users(external_idp_id) WHERE external_idp_id IS NOT NULL;

-- Session tokens minted after a successful OIDC dance. Same SHA-256 storage
-- scheme as tenant_api_tokens so the auth extractor can check both with the
-- same hashing path.
CREATE TABLE user_sessions (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id     UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    user_id       UUID        NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    hashed_token  TEXT        NOT NULL UNIQUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    expires_at    TIMESTAMPTZ NOT NULL,
    revoked_at    TIMESTAMPTZ
);

CREATE INDEX idx_sessions_tenant_user ON user_sessions(tenant_id, user_id);
CREATE INDEX idx_sessions_active      ON user_sessions(tenant_id) WHERE revoked_at IS NULL;
