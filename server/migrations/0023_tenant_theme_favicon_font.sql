-- skill-pool 0023_tenant_theme_favicon_font
-- Issue #9: per-tenant favicon upload + Google-Fonts-allowlist font picker.
--
-- Favicon mirrors the logo column triplet: storage_key + content_type +
-- bytes_size. Three additions vs. logo:
--   1. A smaller 64 KiB cap — favicons are tiny by convention, and a 64 KiB
--      ceiling makes accidental "upload-the-hero-image" mistakes fail loudly.
--   2. The allowed content-types include `image/x-icon` so admins with a
--      classic `.ico` from a brand kit can still use it.
--   3. When `favicon_storage_key IS NULL` the GET /v1/theme/favicon endpoint
--      transparently serves the logo bytes (fallback). That's a pure code
--      decision — no column needed.
--
-- Font picker: `font_family` is a free-form TEXT column rather than an enum
-- because the allowlist lives in code (`ALLOWED_FONTS` in routes/theme.rs)
-- and we want to add/remove fonts without a migration. Server-side validation
-- guards every PUT, so an unsafe value cannot reach the DB through the API.

ALTER TABLE tenant_theme
    ADD COLUMN favicon_storage_key TEXT,
    ADD COLUMN favicon_content_type TEXT,
    ADD COLUMN favicon_bytes_size   INTEGER,
    ADD COLUMN font_family          TEXT;

ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_favicon_content_type_chk
        CHECK (
            favicon_content_type IS NULL OR
            favicon_content_type IN (
                'image/svg+xml',
                'image/png',
                'image/jpeg',
                'image/webp',
                'image/x-icon'
            )
        );

ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_favicon_bytes_size_chk
        CHECK (
            favicon_bytes_size IS NULL OR
            (favicon_bytes_size >= 0 AND favicon_bytes_size <= 65536)
        );

-- Triplet invariant: storage_key set iff content_type and bytes_size are set.
-- Mirrors the logo constraint so the GET endpoint never has to handle a
-- half-populated row.
ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_favicon_storage_triplet_chk
        CHECK (
            (favicon_storage_key IS NULL  AND favicon_content_type IS NULL AND favicon_bytes_size IS NULL) OR
            (favicon_storage_key IS NOT NULL AND favicon_content_type IS NOT NULL AND favicon_bytes_size IS NOT NULL)
        );

-- font_family validation happens in application code (`ALLOWED_FONTS` in
-- routes/theme.rs). We keep a sanity length cap here so a stray value can't
-- bloat a row.
ALTER TABLE tenant_theme
    ADD CONSTRAINT tenant_theme_font_family_len_chk
        CHECK (font_family IS NULL OR length(font_family) <= 64);
