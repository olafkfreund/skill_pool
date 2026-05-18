-- skill-pool 0004_saml
-- Per-tenant SAML 2.0 service-provider config.
--
-- The full ACS (assertion consumer service) handler lands in a follow-up
-- PR — it needs XML signature validation against the IdP cert, which is
-- a non-trivial piece of work better isolated from this scaffolding.
-- This migration + the metadata endpoint built on it unblock customer
-- IdP-side integration work today.

CREATE TABLE tenant_saml (
    tenant_id        UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    -- IdP side (paste from the IdP's metadata XML or admin UI)
    idp_entity_id    TEXT        NOT NULL,
    idp_sso_url      TEXT        NOT NULL,
    idp_x509_cert    TEXT        NOT NULL,   -- PEM, used to validate signed responses
    -- SP side — sp_entity_id defaults to `urn:skill-pool:tenant:<slug>` if NULL.
    sp_entity_id     TEXT,
    default_role     TEXT        NOT NULL DEFAULT 'viewer'
                                 CHECK (default_role IN ('viewer', 'publisher', 'curator', 'admin')),
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TRIGGER tenant_saml_touch_updated_at
    BEFORE UPDATE ON tenant_saml
    FOR EACH ROW EXECUTE FUNCTION touch_updated_at();
