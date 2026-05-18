-- skill-pool 0008_skill_drafts
-- Phase 4 retrospective capture: drafts are skills awaiting curator review.
-- A draft is published by promoting it into the `skills` table inside a
-- transaction that flips `status='published'` on the draft.
--
-- Drafts do NOT have a version — the curator assigns one at publish time.
-- They have their own bundle in storage so a discarded draft is a single
-- DELETE + a single storage purge.

CREATE TABLE skill_drafts (
    id              UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    -- Proposed slug. Editable in the inbox before publish.
    slug            TEXT        NOT NULL,
    description     TEXT        NOT NULL,
    when_to_use     TEXT,
    tags            TEXT[]      NOT NULL DEFAULT '{}',
    -- Where it came from. Phase 4 only `cli`. Phase 4.5 adds `capture-scorer`,
    -- `claude-hook`, `web`.
    origin          TEXT        NOT NULL DEFAULT 'cli'
                                CHECK (origin IN ('cli', 'capture-scorer', 'claude-hook', 'web')),
    -- Free-form reviewer note (why this matters, what session this came from).
    notes           TEXT,
    -- Lifecycle. `pending` is the inbox; `published` means promoted to skills;
    -- `discarded` means a curator rejected it (kept for telemetry, not shown).
    status          TEXT        NOT NULL DEFAULT 'pending'
                                CHECK (status IN ('pending', 'published', 'discarded')),
    -- Pointers to the published skill (if any).
    published_skill_id UUID     REFERENCES skills(id) ON DELETE SET NULL,
    published_version  TEXT,
    -- Bundle on storage. Same opendal backend as skills.
    bundle_uri      TEXT        NOT NULL,
    bundle_sha256   TEXT        NOT NULL,
    -- Audit trail.
    created_by      UUID        REFERENCES users(id) ON DELETE SET NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    reviewed_by     UUID        REFERENCES users(id) ON DELETE SET NULL,
    reviewed_at     TIMESTAMPTZ
);

-- Inbox query: tenant scoped, pending first, newest first.
CREATE INDEX idx_skill_drafts_tenant_status_recent
    ON skill_drafts(tenant_id, status, created_at DESC);

-- Dedup lookup by slug while pending.
CREATE INDEX idx_skill_drafts_tenant_slug
    ON skill_drafts(tenant_id, slug)
    WHERE status = 'pending';
