-- skill-pool 0020_tenant_theme_logo
-- Issue #9: per-tenant logo upload with strict SVG sanitization.
--
-- The pre-existing `logo_uri` column stays put: operators may still wish to
-- point at an external CDN URL instead of uploading bytes through us. When
-- `logo_storage_key IS NOT NULL` the GET /v1/theme/logo endpoint serves the
-- uploaded bytes; otherwise the client falls back to `logo_uri`.
--
-- Size cap (256 KiB) is enforced at three places:
--   1. CHECK constraint here (defence in depth — catches direct SQL writes)
--   2. The multipart handler in routes/theme.rs
--   3. The body-limit middleware that wraps the router
--
-- Allowed content types are the four formats we sanitize / magic-check.

ALTER TABLE tenant_theme
    ADD COLUMN logo_storage_key TEXT,
    ADD COLUMN logo_content_type TEXT,
    ADD COLUMN logo_bytes_size   INTEGER;

ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_logo_content_type_chk
        CHECK (
            logo_content_type IS NULL OR
            logo_content_type IN ('image/svg+xml', 'image/png', 'image/jpeg', 'image/webp')
        );

ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_logo_bytes_size_chk
        CHECK (
            logo_bytes_size IS NULL OR
            (logo_bytes_size >= 0 AND logo_bytes_size <= 262144)
        );

-- All three columns are linked: storage_key being set implies the other two
-- must also be set. Avoids ambiguity in the GET endpoint about how to serve.
ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_logo_storage_triplet_chk
        CHECK (
            (logo_storage_key IS NULL  AND logo_content_type IS NULL AND logo_bytes_size IS NULL) OR
            (logo_storage_key IS NOT NULL AND logo_content_type IS NOT NULL AND logo_bytes_size IS NOT NULL)
        );
