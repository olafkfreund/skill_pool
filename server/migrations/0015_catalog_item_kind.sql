-- skill-pool 0015_catalog_item_kind
-- Phase 5: agents + commands as parallel surfaces to skills.
--
-- A discriminator column on `skills` rather than two new tables. Every
-- Phase 5 feature (dependencies, embeddings, decay, usage, drafts) FKs
-- to skills.id and works automatically across all three kinds. A future
-- rename `skills` → `catalog_items` is a follow-up cleanup; the column
-- here is the cheap win.

ALTER TABLE skills
    ADD COLUMN kind TEXT NOT NULL DEFAULT 'skill'
        CHECK (kind IN ('skill', 'agent', 'command'));

-- Composite index for catalog listing — most queries filter
-- (tenant_id, kind, status, slug) and order by created_at.
CREATE INDEX idx_skills_tenant_kind_slug
    ON skills(tenant_id, kind, slug);
