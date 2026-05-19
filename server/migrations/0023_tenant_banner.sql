-- skill-pool 0023_tenant_banner
-- Phase 5+ / Enterprise: per-tenant CLI startup banner (#9).
--
-- The CLI fetches `/v1/tenant/profile/banner` once per shell session
-- (with a 24h local-mtime cache) and prints `banner_text` (and the
-- optional URL on the line below) to stderr before running the user's
-- subcommand. This gives operators a tiny escape hatch for "welcome to
-- Acme's skill registry — internal docs at https://wiki/skills" without
-- needing to ship custom CLI builds per tenant.
--
-- Length cap (240 chars): comfortable for a tenant name, a one-line
-- greeting, and a short note. Anything longer belongs in the URL
-- destination, not the banner itself. The CHECK enforces the cap at
-- the DB level so the route/admin helper can stay simple.
--
-- URL constraint: must be `https://` and contain no whitespace. Plain
-- HTTP is rejected because CLI users routinely paste these into shells
-- / browsers and an http:// URL in a terminal is a phishing footgun.
-- The whitespace check stops a banner from accidentally spanning two
-- lines via a copy-paste with a stray newline.
ALTER TABLE tenants
    ADD COLUMN banner_text TEXT,
    ADD COLUMN banner_url  TEXT;

ALTER TABLE tenants
    ADD CONSTRAINT tenants_banner_text_len_chk
    CHECK (banner_text IS NULL OR (length(banner_text) >= 1 AND length(banner_text) <= 240));

ALTER TABLE tenants
    ADD CONSTRAINT tenants_banner_url_scheme_chk
    CHECK (banner_url IS NULL OR banner_url ~ '^https://[^[:space:]]+$');
