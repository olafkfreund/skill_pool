-- skill-pool 0014_email_notifications
-- Phase 5: per-tenant SMTP delivery, alongside the webhook surface
-- from migration 0010.
--
-- We don't bundle an SMTP relay — operators wire their own (Postfix,
-- SES, Mailgun, SendGrid, etc.). The URL field accepts standard syntax:
--   smtp://user:pass@host:587     plain SUBMIT
--   smtps://user:pass@host:465    implicit TLS
-- TLS / STARTTLS handling is driven by the URL scheme.

ALTER TABLE tenants
    ADD COLUMN notification_smtp_url  TEXT,
    ADD COLUMN notification_smtp_from TEXT,
    ADD COLUMN notification_smtp_to   TEXT;
