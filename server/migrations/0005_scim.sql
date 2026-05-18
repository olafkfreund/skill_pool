-- skill-pool 0005_scim
-- SCIM 2.0 surface area.
--
-- SCIM resources need a single-column id; `tenant_users` has been a composite
-- (tenant_id, user_id) until now. Adds a synthetic UUID per membership.
--
-- Adds an `active` flag to users so SCIM can deprovision without losing
-- audit history.

ALTER TABLE users ADD COLUMN active BOOLEAN NOT NULL DEFAULT true;

ALTER TABLE tenant_users
    ADD COLUMN id UUID UNIQUE NOT NULL DEFAULT gen_random_uuid();

CREATE INDEX idx_tenant_users_id ON tenant_users(id);
