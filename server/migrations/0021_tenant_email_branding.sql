-- skill-pool 0021_tenant_email_branding
-- Phase 5+: per-tenant branded transactional email with per-tenant SMTP
-- relay. Closes issue #9 — branded transactional emails.
--
-- The existing notification_smtp_url / notification_smtp_from / etc.
-- columns on `tenants` (added in 0014) remain as the simple-mode global
-- target ("send curators a digest"). This new table is the *Enterprise*
-- knob: a fully isolated per-tenant SMTP transport plus branded
-- From/Reply-To/footer. When a row exists here, all transactional mail
-- *to that tenant's recipients* is sent through this transport instead
-- of the global relay.
--
-- Why a separate table (vs more columns on `tenants`):
--   * The 0014 columns describe *where* the global digest goes; this
--     row describes *how* the tenant's outbound branding looks. They're
--     orthogonal and many enterprise tenants will set this one without
--     touching 0014.
--   * Lets us narrow access — branding rows can have RLS / separate
--     audit semantics later without affecting the wider `tenants` row.
--
-- Password storage:
--   smtp_password_enc is AES-256-GCM ciphertext. Key sourced from the
--   SKILL_POOL_EMAIL_SECRET_KEY env var (32 raw bytes, hex-encoded).
--   When that env is unset the server falls back to base64-of-plaintext
--   with a loud warning in the logs — fine for dev / single-tenant
--   self-host, but production deployments MUST set the key.

CREATE TABLE tenant_email_branding (
    tenant_id          UUID        PRIMARY KEY REFERENCES tenants(id) ON DELETE CASCADE,
    from_addr          TEXT        NOT NULL,
    from_name          TEXT,
    reply_to           TEXT,
    smtp_url           TEXT        NOT NULL,
    smtp_password_enc  BYTEA       NOT NULL,
    footer_html        TEXT,
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now()
);

ALTER TABLE tenant_email_branding
    ADD CONSTRAINT tenant_email_branding_smtp_scheme_chk
    CHECK (smtp_url LIKE 'smtp://%' OR smtp_url LIKE 'smtps://%');
