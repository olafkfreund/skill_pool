-- skill-pool 0013_skill_usage_events
-- Phase 5: per-event usage log.
--
-- Sits alongside the per-row `skills.use_count` + `last_used_at` counters
-- from migration 0011. The counter answers "is anyone using this skill?"
-- The event log answers time-windowed questions: "downloads this week",
-- "trending skills last month", "usage by hour over a release window".
--
-- Two event kinds for v1:
--   - download : GET /v1/skills/{slug}/bundle.tar.gz
--   - view     : GET /v1/skills/{slug}/skill-md
--
-- Future kinds (search hits, archive, etc.) layer on without schema
-- change since `event_kind` is opaque TEXT with a CHECK.
--
-- High-write table: BIGSERIAL primary key, partial index optimised for
-- the timeline query (tenant_id + ts range). A monthly partition is a
-- Phase 5+ upgrade when row counts cross ~10M.

CREATE TABLE skill_usage_events (
    id          BIGSERIAL   PRIMARY KEY,
    tenant_id   UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    skill_id    UUID        NOT NULL REFERENCES skills(id) ON DELETE CASCADE,
    event_kind  TEXT        NOT NULL
                            CHECK (event_kind IN ('download', 'view')),
    user_id     UUID        REFERENCES users(id) ON DELETE SET NULL,
    token_id    UUID        REFERENCES tenant_api_tokens(id) ON DELETE SET NULL,
    ts          TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Timeline + top queries hit the same (tenant_id, ts) prefix.
CREATE INDEX idx_skill_usage_events_tenant_ts
    ON skill_usage_events(tenant_id, ts DESC);
-- Per-skill drill-down (deferred to a later UI slice; cheap index now).
CREATE INDEX idx_skill_usage_events_skill_ts
    ON skill_usage_events(skill_id, ts DESC);
