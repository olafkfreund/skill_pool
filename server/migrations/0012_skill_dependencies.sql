-- skill-pool 0012_skill_dependencies
-- Phase 5: dependency resolution.
--
-- A skill can declare other skills it requires via the `requires` field
-- in its SKILL.md frontmatter. Each entry becomes one row here.
--
-- Forward references are allowed: A can be published before B even if A
-- requires B. The /deps endpoint resolves at read time; missing target
-- slugs are surfaced as broken edges in the closure result.
--
-- Version range stored as opaque text: v1 understands `*` (any) and
-- exact `X.Y.Z`. Anything else is stored verbatim and the client picks
-- "latest" if it doesn't recognize the syntax.

CREATE TABLE skill_dependencies (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    parent_skill_id UUID        NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    requires_slug   TEXT        NOT NULL,
    version_range   TEXT        NOT NULL DEFAULT '*',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (parent_skill_id, requires_slug)
);

CREATE INDEX idx_skill_deps_parent
    ON skill_dependencies(parent_skill_id);
-- Used by the recursive CTE join in the closure query.
CREATE INDEX idx_skill_deps_tenant_requires
    ON skill_dependencies(tenant_id, requires_slug);
