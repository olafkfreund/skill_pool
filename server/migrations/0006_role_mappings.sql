-- skill-pool 0006_role_mappings
-- Per-tenant IdP group → tenant role mappings.
--
-- On sign-in, the OIDC / SAML handlers compute the user's effective role
-- as MAX(role of any mapped group the user belongs to). When no groups
-- match (or the assertion didn't carry groups), the existing membership
-- row is preserved — so manual promotions via the members admin page
-- aren't clobbered just because the IdP didn't send a groups claim this
-- time.

CREATE TABLE tenant_role_mappings (
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    idp_group   TEXT        NOT NULL,
    role        TEXT        NOT NULL
                            CHECK (role IN ('viewer', 'publisher', 'curator', 'admin')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, idp_group)
);

CREATE INDEX idx_role_mappings_tenant ON tenant_role_mappings(tenant_id);
