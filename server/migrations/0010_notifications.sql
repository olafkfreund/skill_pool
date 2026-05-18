-- skill-pool 0010_notifications
-- Phase 5: curator notifications for the drafts inbox.
--
-- One webhook URL per tenant for v1 (Slack/Discord/custom). Fanout to
-- multiple destinations is a Phase 5+ layer — the table is shaped so it
-- can be split into a `tenant_webhooks` child table later without losing
-- existing data.
--
-- `notifications_webhook_secret` is optional. When set, the server signs
-- the body with HMAC-SHA256 and ships the hex digest in
-- `X-Skill-Pool-Signature: sha256=<hex>` — matches GitHub/Stripe.

ALTER TABLE tenants
    ADD COLUMN notifications_webhook_url TEXT,
    ADD COLUMN notifications_webhook_secret TEXT;
