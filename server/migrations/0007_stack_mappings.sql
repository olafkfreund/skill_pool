-- skill-pool 0007_stack_mappings
-- Curated stack-tag → skill-slug mappings per tenant.
--
-- A project that fingerprints as ["rust", "axum", "postgres", "nixos"] gets
-- the union of all skills mapped to any of those tags, deduped, capped at 8.
-- This is the "curated mapping" tier described in the plan; tag-intersection
-- and embedding similarity tiers come later.

CREATE TABLE tenant_stack_mappings (
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    stack_tag   TEXT        NOT NULL,
    skill_slug  TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, stack_tag, skill_slug)
);

CREATE INDEX idx_stack_mappings_tenant_tag
    ON tenant_stack_mappings(tenant_id, stack_tag);
