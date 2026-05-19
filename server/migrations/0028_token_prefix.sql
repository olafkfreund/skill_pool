-- Personal API token UI (#4): record a short, non-secret prefix at mint time
-- so the management UI can show users which row corresponds to which copy of
-- the token they pasted into a script. The full raw token is only shown once
-- at creation; the prefix is the durable display affordance.
--
-- Stored as the literal first 12 chars of the raw token (e.g. `spk_1a2b3c4d`).
-- 12 chars on top of a 256-bit secret leaves ~244 bits — well above any
-- guessability concern, and the prefix alone is not sufficient to authenticate
-- (the hashed_token UNIQUE column still gates auth lookups).
--
-- Existing rows: NULL prefix. The UI shows "—" for those; the operator can
-- mint a replacement if they care about the display affordance.
ALTER TABLE tenant_api_tokens
    ADD COLUMN token_prefix TEXT;

-- Lookup-by-user index. Sparse — only API tokens minted via the profile UI
-- carry `created_by`; CLI-minted bootstrap tokens leave it NULL.
CREATE INDEX idx_tokens_creator
    ON tenant_api_tokens(tenant_id, created_by)
    WHERE created_by IS NOT NULL;
