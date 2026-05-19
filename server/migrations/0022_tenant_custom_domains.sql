-- Per-tenant custom domain mappings (Phase 5 / Enterprise).
--
-- Lets a tenant admin map their own hostname (e.g. `skills.acme.com`) at
-- the same backend that normally serves `acme.skill-pool.example.com`.
-- Tenant resolution checks this table BEFORE the subdomain / header
-- fallback (`tenant.rs::slug_from_request`).
--
-- Status flow:
--   pending  → row freshly created; verification TXT record not yet seen
--   verified → DNS TXT confirmed; reverse proxy may now serve certs for it
--   active   → cert issued + first request served (operator-managed)
--   failed   → DNS lookup failed or token mismatch; `last_error` set.
--
-- The CHECK keeps unknown statuses out of the column so we can switch on
-- it without a default arm.
--
-- A hostname can map to exactly one tenant (UNIQUE on hostname): if Acme
-- claims `skills.example.com` they own it for as long as the row exists.
-- DELETE clears it for the next claimant.
CREATE TABLE tenant_custom_domains (
    id                  UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id           UUID        NOT NULL REFERENCES tenants(id) ON DELETE CASCADE,
    hostname            TEXT        NOT NULL,
    status              TEXT        NOT NULL DEFAULT 'pending'
                                    CHECK (status IN ('pending', 'verified', 'active', 'failed')),
    -- Verification: the tenant adds a TXT record at
    -- `_skill-pool-verify.<hostname>` whose value is this token. The
    -- token is random hex (32 bytes → 64 chars) so it can't be guessed.
    verification_token  TEXT        NOT NULL,
    last_checked_at     TIMESTAMPTZ,
    last_error          TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    activated_at        TIMESTAMPTZ
);

CREATE UNIQUE INDEX tenant_custom_domains_hostname_uq ON tenant_custom_domains(hostname);
CREATE INDEX tenant_custom_domains_tenant_idx        ON tenant_custom_domains(tenant_id);
-- Used by the per-process refresh job that loads only active rows into
-- the in-memory host→tenant cache.
CREATE INDEX tenant_custom_domains_status_idx        ON tenant_custom_domains(status)
    WHERE status IN ('verified', 'active');
