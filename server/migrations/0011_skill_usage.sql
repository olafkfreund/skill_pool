-- skill-pool 0011_skill_usage
-- Phase 5: decay tracking.
--
-- Adds `last_used_at` and `use_count` directly on `skills` so we can
-- cheaply identify stale skills without joining a separate events
-- table. A richer `skill_usage_events` table for per-user/per-project
-- telemetry is a Phase 5+ upgrade (not needed for the v1 decay
-- question — "is this skill ever invoked?").
--
-- Backfill: existing rows get `last_used_at = created_at` so that the
-- 180-day-stale heuristic is honest about how long the skill has been
-- around, not just how long since column was added.

ALTER TABLE skills
    ADD COLUMN last_used_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    ADD COLUMN use_count    INTEGER     NOT NULL DEFAULT 0;

-- Existing rows: treat their birth as the last-used baseline so we
-- don't insta-decay every catalog on first deploy. The ALTER above
-- already set last_used_at = now() for them; override to created_at
-- so decay reflects actual skill age.
UPDATE skills SET last_used_at = created_at;

-- Partial index for the decay query: published skills are the only
-- decay candidates, and the column is small so the index is cheap.
CREATE INDEX idx_skills_decay
    ON skills(tenant_id, last_used_at, use_count)
    WHERE status = 'published';
