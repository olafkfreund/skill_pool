# SCIM 2.0 — automated user provisioning

skill-pool implements the subset of SCIM 2.0 that Okta, Azure AD / Entra,
and Google Workspace actually exercise in their lifecycle-management
integrations: user create, lookup-by-userName, deactivate (PATCH active=false),
and delete.

## 1. Mint a provisioning token

Each tenant gets its own provisioning token. From the box running the server:

```bash
skill-pool-server admin token-create \
  --tenant acme \
  --name okta-provisioning \
  --scope 'scim:provision'
```

The raw token (`spk_…`) is printed **once**. Hand it to the IdP admin; it
goes into the SCIM connector configuration as the Bearer token.

## 2. Point the IdP at the SCIM endpoint

| IdP | Setting | Value |
|---|---|---|
| Okta | SCIM connector base URL | `https://<your-host>/scim/v2` |
| Okta | Authentication mode | HTTP Header — `Authorization: Bearer <token>` |
| Okta | Provisioning actions | Create Users, Update User Attributes, Deactivate Users |
| Azure AD | Tenant URL | `https://<your-host>/scim/v2` |
| Azure AD | Secret Token | the `spk_…` token |
| Azure AD | Provisioning attributes | userName (required), active, emails |

If your portal sits on a tenant subdomain, the IdP also needs to send the
tenant slug — add the request header `X-Skill-Pool-Tenant: acme` in
the connector's HTTP headers. Or use the matching subdomain (e.g.
`acme.skill-pool.example.com/scim/v2`) and the server resolves it from
`Host`.

## 3. Supported endpoints

| Endpoint | Method | Behaviour |
|---|---|---|
| `/scim/v2/ServiceProviderConfig` | GET | Capability announcement (filter+patch on, bulk off) |
| `/scim/v2/ResourceTypes` | GET | Lists the `User` resource type |
| `/scim/v2/Schemas` | GET | Returns the minimal User schema |
| `/scim/v2/Users` | GET | Lists memberships; supports `?filter=userName eq "x"` |
| `/scim/v2/Users` | POST | Creates a user + adds membership at `viewer` role |
| `/scim/v2/Users/{id}` | GET | Fetches one membership |
| `/scim/v2/Users/{id}` | PATCH | Only `replace active true/false` is supported |
| `/scim/v2/Users/{id}` | DELETE | Deactivates membership (same effect as PATCH active=false) |

The SCIM resource ID is the `tenant_users.id` (one per tenant×user). A
user signed in to two tenants has two distinct SCIM IDs.

## 4. What "deactivate" does

- Removes the row from `tenant_users` for this tenant (user loses access)
- Sets `users.active = false`
- Revokes all live `user_sessions` for this tenant×user — Okta-style
  deprovisioning kicks the user out of any open browser tabs by the next
  page load

The `users` row is kept (preserves audit trail). If the IdP re-provisions
the same user later, POST `/scim/v2/Users` will re-add membership and
re-activate the user.

## 5. Filter support

Only `userName eq "..."` is supported. That's what Okta and Azure use to
check "does this user already exist before I POST?". Anything else
returns 400.

Future: `id eq "..."`, `active eq true`, `meta.lastModified gt ...` for
delta sync. None of those are needed by today's provisioning lifecycle.

## 6. What's NOT supported (yet)

- **Groups** — out of scope; tenant_users.role is set by admins via the
  CLI or (eventually) the members admin page
- **PATCH ops other than `replace active`** — the full SCIM path-expression
  parser is a lot of code for a small payoff; the deprovisioning op is
  what IdPs care about
- **PUT** — Okta and Azure both use PATCH; no IdP actually exercises PUT
- **Bulk** — explicitly off in ServiceProviderConfig
- **ETag** — off; SCIM clients should re-GET if they care about freshness

These show up in the ServiceProviderConfig so IdPs adapt their integration
plan accordingly.

## 7. Audit

Every POST / PATCH / DELETE hits `audit_events` with a `scim.*` action
prefix; SIEM exports inherit the same filter you set for other tenant
operations. (Wired in the next iteration alongside the OIDC audit pass.)
