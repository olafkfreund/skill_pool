# Session policy

Per-tenant session idle-timeout. Tenants with stricter security postures
(financial services, healthcare, government) can shorten the web
portal's session cookie max-age below the default 14 days without
forcing tighter timeouts on every other tenant.

## What this controls

The web portal's `sp_token` and `sp_tenant` cookies' `maxAge`, set at
login. When the cookie expires, the user is bounced to `/login` and
must paste their token again. The API token itself is not affected —
it lives in `tenant_api_tokens` until explicitly revoked.

## Defaults

- **No policy set** (`tenants.session_max_age_secs IS NULL`): web uses
  its built-in 14-day default.
- **Policy set**: web uses the value verbatim. Range enforced at the
  DB layer: **60 seconds (1 minute) to 2,592,000 seconds (30 days)**.

## Set it up

### Via admin CLI

```bash
# 1 hour for a regulated tenant
skill-pool-server admin tenant-session-policy --slug acme --max-age-days 1

# 7 days for a "team" plan
skill-pool-server admin tenant-session-policy --slug acme --max-age-days 7

# Clear and revert to system default (14 days)
skill-pool-server admin tenant-session-policy --slug acme --clear
```

The CLI clamps to 1..30 days. To go below 1 day (down to the 1-minute
minimum) write directly to the column:

```sql
UPDATE tenants
   SET session_max_age_secs = 3600  -- 1 hour
 WHERE slug = 'acme';
```

(The 1..30 day clamp on the CLI is for ergonomics; the DB CHECK is the
actual policy boundary.)

### Read the current value

```bash
curl https://acme.skill-pool.example.com/v1/tenant/session-policy
```

```json
{
  "max_age_secs": 3600,
  "configured": true
}
```

`configured: true` means the tenant has an explicit policy;
`configured: false` means the value is the system default.

## When it takes effect

**At next login.** Sessions already in flight keep their existing
cookie `maxAge`. This is intentional:

- Tightening the policy applies to new sessions; existing users
  aren't surprise-logged-out mid-task.
- Loosening the policy applies to new logins too; existing cookies
  are still bounded by their original maxAge.

If you need to invalidate every active session immediately (security
incident, departing admin), rotate the per-tenant API tokens via the
admin CLI: every session whose token is revoked loses access on the
next API request.

## Architecture

- **`tenants.session_max_age_secs`** (`server/migrations/0019`) — the
  source of truth. INTEGER, nullable, CHECK 60..2_592_000.
- **`GET /v1/tenant/session-policy`** — no-auth read endpoint the web
  portal calls during the login action. Returns the resolved value
  (custom or default) plus a `configured` flag.
- **`web/src/lib/server/api.ts::getSessionMaxAge`** — wraps the endpoint
  with a 14-day fallback on any error. The login flow never blocks on
  the policy fetch.
- **`web/src/routes/(public)/login/+page.server.ts`** — the login form
  action calls `getSessionMaxAge` and passes the result as the cookie
  `maxAge`.

## Limitations

- **Cookie-side only.** The API token itself has no expiry — the
  session policy bounds the *cookie*, not the credential. Combining
  short cookie lifetime with token rotation gives you a defense-in-
  depth policy.
- **No idle-vs-absolute distinction.** This is a fixed cookie `maxAge`
  from login, not a "logout after N minutes of inactivity" sliding
  window. SvelteKit's cookies don't support sliding TTL without a
  per-request rewrite middleware, which would add a write to every
  request. Re-evaluate if customer demand pushes us toward sliding.
- **Per-tenant only, not per-user.** Inside a tenant every user gets
  the same policy. Per-user policies would need a `users` column
  (joined on the IdP-provisioned user row).
- **Web-only.** The MCP transport (`POST /v1/mcp`) and the CLI use
  bearer tokens directly — they're not affected by session policy.

## Related

- `server/migrations/0019_tenant_session_policy.sql` — schema
- `server/src/routes/session_policy.rs` — endpoint
- `server/src/admin.rs::set_session_max_age` — admin helper
- `server/tests/session_policy.rs` — end-to-end test
- `docs/enterprise/data-residency.md` — its sibling per-tenant policy
- `docs/sso.md` — SSO docs cover the IdP-side timeouts that often
  pair with this setting
