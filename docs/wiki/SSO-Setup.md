# SSO Setup

> skill-pool supports two SSO mechanisms, configurable per tenant.
> Both ship end-to-end working in Phase 2. Pick OIDC if your IdP
> supports it (almost all modern ones do); fall back to SAML 2.0
> only for ADFS, on-prem Shibboleth, or legacy Okta/Azure SAML apps.

| Mechanism | Status | Use it for |
|---|---|---|
| **OIDC** | end-to-end | Okta, Azure AD, Google Workspace, Authentik, Keycloak, Auth0, Ping — anything OIDC Authorization Code + PKCE |
| **SAML 2.0** | end-to-end (XML sig validation via xmlsec1) | ADFS, on-prem Shibboleth, older Okta/Azure SAML apps |

Two ways to configure both: the admin CLI on the server host, or the
admin SSO UI shipped in #4 (see screenshot reference below).

---

## Admin UI walkthrough

The admin SSO config page lives at `/admin/sso` in the portal. It's
gated behind `tenant:admin`. Two tabs — OIDC and SAML — each with a
read-only "current config" view plus a form to set or update.

![SSO admin page](https://raw.githubusercontent.com/olafkfreund/skill_pool/main/docs/images/sso.webp)

The page reads/writes:

- `GET /v1/tenant/sso/oidc` — returns redacted current OIDC config
- `PUT /v1/tenant/sso/oidc` — set/update OIDC
- `DELETE /v1/tenant/sso/oidc` — clear OIDC
- `GET /v1/tenant/sso/saml` — returns current SAML config
- `PUT /v1/tenant/sso/saml` — set/update SAML
- `DELETE /v1/tenant/sso/saml` — clear SAML

Validation: PUT runs the same checks as the CLI (issuer
`.well-known/openid-configuration` reachable, PEM parses). On error
the form renders a red banner with the specific failure.

Client-secret display semantics: the API redacts client secrets in
GET responses (returns `null`). The form has a "leave blank to keep
existing" affordance — submitting an empty client secret on update
preserves the stored one.

---

## Group → role mappings

On every sign-in (OIDC + SAML), the server reads the user's group
claims and picks the highest-precedence matched role. Order:
`viewer < publisher < curator < admin`.

Configure via the admin CLI:

```bash
skill-pool-server admin group-map-set --tenant acme \
  --group "Engineering-Admins" --role admin
skill-pool-server admin group-map-set --tenant acme \
  --group "Curators" --role curator
skill-pool-server admin group-map-set --tenant acme \
  --group "Engineers" --role publisher

skill-pool-server admin group-map-list --tenant acme
skill-pool-server admin group-map-remove --tenant acme --group "Engineers"
```

Claim names the server looks for:

| Protocol | Claim names (priority order) |
|---|---|
| OIDC | `groups` (Okta, Authentik, Keycloak), `roles` (Azure AD with app-role mapping), `memberOf` (custom mappers) |
| SAML | `<AttributeStatement>` named `groups`, `memberOf`, or `Role` |

Key properties:

- **Highest role wins.** Mary in `Engineering-Admins` + `Engineers` → `admin`.
- **No-match preserves the existing role.** If the IdP omits groups or
  sends groups that don't match any mapping, the membership row isn't
  touched. Manual promotions via the members admin page survive
  sign-ins that lack group claims.
- **Downgrades propagate when groups ARE present.** Remove a user from
  `Engineering-Admins` in Okta → next sign-in drops their role.

IdP-side setup tips:

- **Okta** — App → Sign On → "Groups attribute statement". `Name =
  groups`. Filter = "Matches regex `.*`" or scope as desired.
- **Azure AD / Entra** — App registration → Token configuration → Add
  groups claim. Default emits object IDs; configure
  "sAMAccountName" if you want names matching the mapping table.
- **Authentik** — Property mapping → return `groups` as a list of
  human-readable names.
- **Google Workspace** — `groups` claim isn't on the OIDC token by
  default; needs Cloud Identity Groups Toolkit integration.

---

## OIDC

### 1. Register skill-pool as an OIDC client in your IdP

| Setting | Value |
|---|---|
| Sign-in redirect URI | `https://<your-skill-pool-host>/v1/auth/oidc/<tenant-slug>/callback` |
| Sign-out redirect URI | `https://<your-skill-pool-host>/login` (optional) |
| Grant types | Authorization Code |
| Response types | Code |
| PKCE | required |
| Scopes | `openid`, `email`, `profile` |

Capture the resulting **issuer URL**, **client ID**, and
**client secret**.

### 2. Configure the tenant

Via CLI:

```bash
skill-pool-server admin sso-set \
  --tenant acme \
  --issuer 'https://acme.okta.com/oauth2/default' \
  --client-id 'YOUR_CLIENT_ID' \
  --client-secret 'YOUR_CLIENT_SECRET' \
  --default-role publisher
```

Or via the admin UI at `/admin/sso` — OIDC tab.

### 3. Test

Visit `https://acme.skill-pool.example.com/login` — you should see a
**"Sign in with SSO (OIDC)"** button. Click it; round-trip through
your IdP; land in the catalog.

### IdP-specific notes

- **Okta** — Use the OIDC > Web application template; tick
  "Authorization Code".
- **Azure AD / Entra** — Create an App registration; under
  Authentication add the redirect URI as a "Web" platform; in
  Certificates & secrets create a client secret. Issuer URL is
  `https://login.microsoftonline.com/<tenant-id>/v2.0`.
- **Google Workspace** — GCP Console → APIs & Services → Credentials
  → OAuth client ID (Web application). Issuer is
  `https://accounts.google.com`.
- **Authentik** — Applications → Providers → OAuth2/OpenID →
  SP-initiated. Issuer is
  `https://authentik.example.com/application/o/<slug>/`.

### Common OIDC errors

| Symptom | Likely cause |
|---|---|
| `400 issuer not reachable` on PUT | Firewall blocking server → IdP; check egress. |
| `redirect_uri mismatch` from IdP | Trailing slash on the registered URI; must match exactly. |
| Sign-in works but role is wrong | Group claim missing or empty; check IdP claim configuration. |
| `nonce mismatch` | PKCE state cookie blocked; check `kit.csrf.trustedOrigins` in the web. |

---

## SAML 2.0

### 1. Hand the IdP admin our SP metadata URL

```
https://<your-skill-pool-host>/v1/auth/saml/<tenant-slug>/metadata
```

They import that URL into their IdP. It declares:

- Our SP entity ID (default: `urn:skill-pool:tenant:<slug>`; overridable)
- Our ACS URL (`POST /v1/auth/saml/<tenant>/acs`)
- HTTP-POST binding
- NameID format: `emailAddress`
- We require **signed assertions**

### 2. Get the IdP's signing certificate + SSO URL

From the IdP, capture:

- IdP entity ID (URI)
- SSO URL (where users go to authenticate)
- X.509 signing certificate (PEM, including BEGIN/END markers)

Save the cert as `idp.pem` somewhere readable.

### 3. Configure the tenant

```bash
skill-pool-server admin saml-set \
  --tenant acme \
  --idp-entity-id 'https://acme.okta.com/exk...' \
  --idp-sso-url 'https://acme.okta.com/app/.../sso/saml' \
  --idp-cert-path /path/to/idp.pem \
  --default-role publisher
```

Or via the admin UI at `/admin/sso` — SAML tab. Paste the cert PEM
into the textarea.

### 4. Test

SAML is **IdP-initiated** in v1. Users go to their IdP portal, click
the skill-pool application tile, and the IdP POSTs the signed
assertion to our ACS endpoint. The server:

1. Base64-decodes the `SAMLResponse` form field.
2. Validates the XML signature against the stored IdP certificate
   (via `samael` → libxml2 + xmlsec1).
3. Checks `Conditions/NotOnOrAfter`.
4. Pulls `email` from NameID or `email`/`mail`/`Email` attributes,
   `displayName` from `displayName`/`name`/(`givenName` + `surname`).
5. Upserts the user, ensures membership at `default_role`, mints a
   14-day session, redirects to `/saml-return?token=…&tenant=…`.

SP-initiated SAML (generating `<AuthnRequest>`) is a follow-up —
IdP-initiated is what every modern IdP supports natively and what
enterprise integrations default to.

### Runtime dependencies (server host)

Signature validation needs xmlsec1 + libxml2. The server's Docker
image installs them; for non-Docker deploys:

- Debian/Ubuntu: `apt install libxml2 libxmlsec1 libxmlsec1-openssl`
- Nix: bundled in `flake.nix`'s dev shell and
  `nixosModules.skill-pool-server`. If you hit a build failure for
  `samael`, ensure both `libxml2` and `xmlsec` are in your devshell
  inputs — see [FAQ](FAQ.md).

### IdP-specific notes

- **Okta** — SAML 2.0 app integration. Set the Single Sign On URL =
  our ACS URL, Audience URI = our SP entity ID, NameID format =
  EmailAddress.
- **Azure AD / Entra** — Enterprise application → Set up single
  sign-on → SAML. Identifier (Entity ID) = our SP entity ID, Reply
  URL = our ACS URL.
- **ADFS** — Add Relying Party Trust → import federation metadata
  from our metadata URL.

### Common SAML errors

| Symptom | Likely cause |
|---|---|
| `signature validation failed` | Wrong PEM uploaded, or IdP rotated cert and you haven't pulled the new one. |
| `assertion expired` | Clock skew between IdP and server > 5 min. Enable NTP. |
| `no email attribute found` | IdP isn't sending email in NameID or any of the recognized attributes; adjust attribute mappings. |
| `Audience does not match` | SP entity ID mismatch — check both ends agree on `urn:skill-pool:tenant:<slug>`. |

---

## Session lifetime

OIDC sessions last **14 days**. There's no refresh-token rotation in
this scaffold — users re-OIDC on expiry. SAML sessions follow the
same path once the ACS handler authenticates.

API tokens (`spk_…`) minted via `admin token-create` are unaffected
and remain the canonical mechanism for the CLI. Personal tokens
(minted by users from `/profile` — see [API Reference](API-Reference.md#profile-developer-4))
follow the same model.

---

## Idle-timeout policy

A per-tenant idle-timeout policy (Phase 2 / #8 §L29) layered on top of
the 14-day session: a session whose last activity is older than
`idle_timeout_secs` returns 401 and the user must re-SSO. Default
disabled; configure with:

```bash
skill-pool-server admin tenant-session-policy --tenant acme \
  --idle-timeout-secs 3600   # 1 hour
```

Set `0` to disable.

---

## SCIM (provisioning)

SCIM 2.0 (`/v1/scim/v2/Users` + `/v1/scim/v2/Groups`) is wired and
deletes/adds users + groups when the IdP pushes them. Auth is via a
dedicated SCIM bearer token, separate from API tokens:

```bash
skill-pool-server admin scim-token-create --tenant acme --name "okta-scim"
# prints raw token once; paste into the IdP's SCIM config.
```

Full SCIM detail in `docs/scim.md`.

---

## Where to read next

- [Tenant Onboarding](Tenant-Onboarding.md) — full first-time playbook
- [API Reference](API-Reference.md#tenant-sso-admin-4) — `/v1/tenant/sso/*` endpoints
- [Multi-Tenancy](Multi-Tenancy.md) — tenant + token model
- [FAQ](FAQ.md) — NixOS samael build, CSRF errors, host header issues

## Cross-links into the codebase

- `server/src/routes/auth/oidc.rs` — OIDC callback handler
- `server/src/routes/auth/saml.rs` — SAML ACS handler
- `server/src/routes/tenant_sso.rs` — admin endpoints (#4)
- `server/src/admin.rs::sso_set` / `saml_set` — CLI verbs
- `server/migrations/0006_tenant_sso.sql` — schema
- `web/src/routes/(authed)/admin/sso/+page.svelte` — admin UI
- `docs/sso.md` — original SSO note this page mirrors
