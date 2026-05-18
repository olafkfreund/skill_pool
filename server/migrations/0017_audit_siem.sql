-- skill-pool 0016_audit_siem
-- Phase 5: SIEM export — fan out audit_events to a tenant-configured
-- HTTPS receiver. Splunk HEC, Datadog Logs, and any "POST JSON over
-- bearer auth" receiver share a shape, so a single URL+token pair
-- covers all three. One row = one POST; payload mirrors the audit row.
--
-- Token is sent as `Authorization: Bearer <token>` when set. The GET
-- endpoint never returns the token verbatim — admins see a boolean
-- "token configured" indicator instead.
--
-- Deliveries are best-effort: a failing receiver MUST NOT block the
-- inflight request, and we intentionally do NOT re-audit delivery
-- outcomes (unlike the curator webhook) to avoid feedback loops where
-- a SIEM POST itself would produce another audit row.

ALTER TABLE tenants
    ADD COLUMN tenant_audit_siem_url   TEXT,
    ADD COLUMN tenant_audit_siem_token TEXT;
