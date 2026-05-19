# Branded transactional emails

Per-tenant SMTP relay and From-line branding for transactional email. An
enterprise tenant on `acme.example.com` can have curator-notification
emails sent from `noreply@acme.example.com` through their own SES /
Postmark / SendGrid / Postfix relay — not yours.

## What this gives you

- **Per-tenant From line** — display name, address, and optional
  Reply-To override.
- **Per-tenant SMTP relay** — each tenant uses its own credentials
  against its own provider. Failures of one tenant's mail provider
  cannot affect another tenant's delivery.
- **At-rest password encryption** — SMTP passwords are stored
  AES-256-GCM-encrypted under an operator-managed key, never as
  plaintext.
- **Optional plain-text footer** — appended to outbound mail (think
  "internal communications" or a compliance disclaimer).

When a tenant has no branding row, transactional mail falls through to
the existing global SMTP path configured by the `tenants.notification_smtp_*`
columns (migration `0014`). Behaviour for existing tenants is unchanged.

## Encryption setup (production REQUIRED)

The server reads a 32-byte AES-256-GCM key from the
`SKILL_POOL_EMAIL_SECRET_KEY` env var, hex-encoded (64 hex chars).

```bash
# Generate once and store it where your secrets live (Vault, agenix,
# k8s Secret, …). Loss of this key means stored passwords cannot be
# decrypted — back it up.
openssl rand -hex 32
```

Set it on every server replica before the next admin-CLI invocation:

```bash
export SKILL_POOL_EMAIL_SECRET_KEY=<64 hex chars>
```

**Without this env set**, the server falls back to storing passwords as
base64-encoded plaintext and emits a `WARN` log on every write. This is
acceptable for local dev and single-tenant self-hosts but **must not**
be used in production. The encrypted-vs-plaintext-fallback format is
distinguished by a leading byte in the stored blob, so you can rotate
between modes without a schema change — but a rotation of the *key
itself* requires a re-PUT of every tenant's branding row.

## Schema

Table `tenant_email_branding` (migration `0021_tenant_email_branding.sql`):

| Column              | Type        | Notes                                                                       |
|---------------------|-------------|-----------------------------------------------------------------------------|
| `tenant_id`         | UUID        | PK; FK to `tenants(id)` with `ON DELETE CASCADE`                            |
| `from_addr`         | TEXT        | e.g. `noreply@acme.example.com`. Validated as an email at write time        |
| `from_name`         | TEXT        | Optional display name, e.g. `Acme Skill Pool`                               |
| `reply_to`          | TEXT        | Optional override; same validation as `from_addr`                           |
| `smtp_url`          | TEXT        | `smtps://user@host:port` or `smtp://user@host:port` — **no password**       |
| `smtp_password_enc` | BYTEA       | AES-256-GCM ciphertext; format byte + 12-byte nonce + tag                   |
| `footer_html`       | TEXT        | Optional text appended to outbound mail (rendered as plain text today)      |
| `updated_at`        | TIMESTAMPTZ | Touched on every upsert                                                     |

A CHECK constraint enforces `smtp_url LIKE 'smtp%://%'`.

## Set it up via the admin CLI

```bash
skill-pool-server admin email-branding-set \
  --tenant acme \
  --from-addr noreply@acme.example.com \
  --from-name "Acme Skill Pool" \
  --reply-to support@acme.example.com \
  --smtp-url smtps://relay@smtp.eu.acme.example.com:465 \
  --footer-html "Acme internal — do not reply"
# Prompts on stderr for the SMTP password (never on the command line).
```

`--from-name`, `--reply-to`, and `--footer-html` are optional. The SMTP
password is read from stdin so it never lands in shell history or
`ps`-visible argv. The CLI prints a `[WARN]` line when
`SKILL_POOL_EMAIL_SECRET_KEY` is unset.

## Set it up via HTTP

All endpoints require the `tenant:admin` scope.

```http
PUT /v1/tenant/email-branding
Authorization: Bearer <admin-token>
Content-Type: application/json

{
  "from_addr": "noreply@acme.example.com",
  "from_name": "Acme Skill Pool",
  "reply_to": "support@acme.example.com",
  "smtp_url": "smtps://relay@smtp.eu.acme.example.com:465",
  "smtp_password": "<the relay password>",
  "footer_html": "Acme internal — do not reply"
}
```

Response shape (note `password_configured` instead of any password
echo):

```json
{
  "from_addr": "noreply@acme.example.com",
  "from_name": "Acme Skill Pool",
  "reply_to": "support@acme.example.com",
  "smtp_url": "smtps://relay@smtp.eu.acme.example.com:465",
  "password_configured": true,
  "footer_html": "Acme internal — do not reply"
}
```

`GET /v1/tenant/email-branding` returns the same shape, or `404` if no
row is configured.

`DELETE /v1/tenant/email-branding` removes the row; the next email send
falls back to the global SMTP path.

## Verify with `/test`

Before relying on a new configuration for real notifications, send a
probe email:

```http
POST /v1/tenant/email-branding/test
Authorization: Bearer <admin-token>
Content-Type: application/json

{ "recipient": "ops@acme.example.com" }
```

Or from the CLI:

```bash
skill-pool-server admin email-branding-test --tenant acme --to ops@acme.example.com
```

The endpoint always returns `200` with a structured `{ "result": "success" | "failed", "error": "…" }`
payload — SMTP reachability is the operator's problem to diagnose, not
a server error. The outcome is also written to `audit_events` with
`action = tenant.email_branding.test`.

## Limitations

- **App-managed key**, not external KMS. For FIPS-compliant deploys or
  HSM-backed key custody, encrypt the column at the storage layer
  (Postgres TDE, encrypted EBS volume) and treat `SKILL_POOL_EMAIL_SECRET_KEY`
  as the second factor. KMS integration is a follow-up.
- **Plain-text bodies only.** `footer_html` is documented as HTML but
  currently rendered as a plain-text appendix. A multipart/HTML build
  is a follow-up — the column lets you set the value now without a
  schema change later.
- **No SPF/DKIM check at write time.** The CLI does not verify that
  your DNS authorises your relay to send from `from_addr`. Send the
  probe via `/test` to a mailbox that runs SPF/DKIM checks before
  going live.
- **Cache invalidates on PUT/DELETE only.** The per-tenant transport
  is cached in-process; changes propagate within the writing
  replica's process immediately, but other replicas pick up the
  change on their next miss. For a fleet-wide rotation, restart
  replicas or rely on the cold-path rebuild.
