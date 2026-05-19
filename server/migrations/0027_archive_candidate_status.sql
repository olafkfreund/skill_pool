-- skill-pool 0027_archive_candidate_status
-- Phase 5 lifecycle: background decay sweep (#7).
--
-- The on-demand `/v1/tenant/skills/decay` endpoint surfaces stale skills
-- to operators on request. The background sweep that ships alongside
-- this migration flips long-stale rows to a new `archive_candidate`
-- status so curators see them flagged proactively — without
-- auto-archiving anything (that stays an explicit admin verb).
--
-- We broaden the CHECK constraint to add the new value. The list /
-- search endpoints filter `status = 'published'`, so candidates do not
-- leak into the catalog. The decay-candidates endpoint surfaces them
-- via the existing `last_used_at / use_count` heuristic; both paths
-- behave identically for the new value.

ALTER TABLE skills
    DROP CONSTRAINT skills_status_check;

ALTER TABLE skills
    ADD CONSTRAINT skills_status_check
    CHECK (status IN ('draft', 'published', 'archived', 'archive_candidate'));
