-- skill-pool 0016_footer_branding
-- Issue #9: "Powered by skill-pool" footer toggle.
-- On = footer credit shown (default, Free tier). Off = hidden.

ALTER TABLE tenant_theme
    ADD COLUMN footer_branding BOOLEAN NOT NULL DEFAULT TRUE;
