-- Per-tenant data residency. `tenants.region` already exists since
-- migration 0001 (free-form tag, "eu-west-1" / "us-east-1" / etc.) but
-- was never wired into business logic. This migration adds the second
-- half: a per-tenant bundle-storage URI override.
--
-- When `storage_uri` is NULL the server uses the global
-- SKILL_POOL_STORAGE_URI. When set, every bundle read/write for this
-- tenant uses the override instead. Existing tenants get NULL: zero
-- behavioural change.
ALTER TABLE tenants
    ADD COLUMN storage_uri TEXT;

-- Loose URI-scheme check so a typo can't silently land. Storage::from_uri
-- also validates at admin-set time; this is belt-and-braces.
ALTER TABLE tenants
    ADD CONSTRAINT tenants_storage_uri_scheme_chk
    CHECK (
        storage_uri IS NULL
        OR storage_uri LIKE 'fs://%'
        OR storage_uri LIKE 's3://%'
    );
