-- skill-pool 0025_tenant_custom_css
-- Issue #9: per-tenant custom CSS overlay with strict sanitization.
--
-- An Enterprise tenant can upload a small CSS file (e.g. final brand polish
-- on top of the curated `--sp-*` variables) that the portal serves at
-- `/v1/theme/custom.css`. The server sanitizes the bytes (`css_sanitize`)
-- before persisting them; the GET endpoint returns the sanitized bytes
-- alongside `Content-Security-Policy: style-src 'self'` so the response
-- itself cannot pull in external stylesheets even if a bypass slipped past
-- the sanitizer.
--
-- Cap (32 KiB) is enforced at three places:
--   1. CHECK constraint here — defends against direct SQL writes.
--   2. The multipart handler in routes/theme.rs.
--   3. The body-limit middleware that wraps the router.
--
-- 32 KiB is generous: a Bootstrap-style brand override is typically ~5-10 KiB
-- minified. Tenants who outgrow it almost certainly want a full custom skin,
-- not an overlay.
--
-- Only two columns this time (vs. logo's three): the content-type is always
-- `text/css; charset=utf-8`, no need to store it.

ALTER TABLE tenant_theme
    ADD COLUMN custom_css_storage_key TEXT,
    ADD COLUMN custom_css_bytes_size  INTEGER;

ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_custom_css_bytes_size_chk
        CHECK (
            custom_css_bytes_size IS NULL OR
            (custom_css_bytes_size >= 0 AND custom_css_bytes_size <= 32768)
        );

-- Pair invariant: storage_key set iff bytes_size is set. Mirrors the logo /
-- favicon "triplet" constraint so the GET endpoint never has to handle a
-- half-populated row.
ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_custom_css_pair_chk
        CHECK (
            (custom_css_storage_key IS NULL  AND custom_css_bytes_size IS NULL) OR
            (custom_css_storage_key IS NOT NULL AND custom_css_bytes_size IS NOT NULL)
        );
