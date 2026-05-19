-- Per-tenant rate-limit overrides. NULL columns mean "use the plan
-- default" (see `rate_limit::default_for_plan`). When set, the column's
-- value wins over the plan default and the value is enforced by the
-- `rate_limit::rate_limit_layer` middleware against a Redis counter
-- keyed by (tenant_id, current_window).
--
-- `rate_limit_rpm`: requests per 60-second window. Plan defaults today:
--   team       → 600  (10 rps sustained)
--   business   → 3000 (50 rps sustained)
--   enterprise → 30000 (500 rps sustained)
--
-- `rate_limit_burst`: requests per 1-second window. Sized to absorb
-- short spikes (CI fan-out, bulk publish) without letting a runaway
-- script ship pathological traffic. Plan defaults: 60 / 300 / 1000.
--
-- Both columns are CHECK-bounded so a typo can't accidentally configure
-- "1 request per minute" (denial-of-service the tenant) or "10 billion
-- requests per minute" (no real throttling). Upper bounds picked well
-- above what any sane workload needs so they're never the bottleneck.
ALTER TABLE tenants
    ADD COLUMN rate_limit_rpm INTEGER,
    ADD COLUMN rate_limit_burst INTEGER;

ALTER TABLE tenants
    ADD CONSTRAINT tenants_rate_limit_rpm_range_chk
    CHECK (rate_limit_rpm IS NULL OR (rate_limit_rpm > 0 AND rate_limit_rpm <= 100000));

ALTER TABLE tenants
    ADD CONSTRAINT tenants_rate_limit_burst_range_chk
    CHECK (rate_limit_burst IS NULL OR (rate_limit_burst > 0 AND rate_limit_burst <= 10000));
