-- Per-tenant session idle timeout. When NULL the web portal uses its
-- built-in default (14 days). When set, the value caps the session
-- cookie's maxAge at login.
--
-- Range: 60 seconds (1 minute) to 2_592_000 seconds (30 days). The
-- lower bound is defensive — operators wanting "log out instantly"
-- should rotate the API token instead of setting a 0-second TTL. The
-- upper bound matches what most security frameworks consider
-- acceptable for non-sensitive workloads; tighter policies are the
-- whole point of this column.
ALTER TABLE tenants
    ADD COLUMN session_max_age_secs INTEGER;

ALTER TABLE tenants
    ADD CONSTRAINT tenants_session_max_age_range_chk
    CHECK (
        session_max_age_secs IS NULL
        OR (session_max_age_secs >= 60 AND session_max_age_secs <= 2592000)
    );
