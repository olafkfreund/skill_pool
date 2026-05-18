# SSO integration

skill-pool supports two SSO mechanisms, configurable per tenant. Both are
managed via the `skill-pool-server admin` CLI; the web portal admin UI for
managing them is part of #8 and lands later.

| Mechanism | Status | Use it for |
|---|---|---|
| **OIDC** | end-to-end working | Okta, Azure AD, Google Workspace, Authentik, Keycloak, Auth0, Ping — anything that speaks OpenID Connect Authorization Code + PKCE |
| **SAML 2.0** | end-to-end working — XML signature validation via xmlsec1 | ADFS, on-prem Shibboleth, older Okta/Azure SAML apps |

Either way, the first user to sign in for a tenant is provisioned with
`tenant_users.role = default_role` (configurable per IdP). Subsequent sign-ins
just refresh the session.

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

Capture the resulting **issuer URL**, **client ID**, and **client secret**.

### 2. Configure the tenant

```bash
skill-pool-server admin sso-set \
  --tenant acme \
  --issuer 'https://acme.okta.com/oauth2/default' \
  --client-id 'YOUR_CLIENT_ID' \
  --client-secret 'YOUR_CLIENT_SECRET' \
  --default-role publisher
```

### 3. Test

Visit `https://acme.skill-pool.example.com/login` — you should see a
"Sign in with SSO (OIDC)" button. Clicking it redirects through your IdP
and lands you back in the catalog.

### IdP-specific notes

- **Okta**: use the OIDC > Web application template; tick "Authorization Code".
- **Azure AD / Entra**: create an App registration; under Authentication add the redirect URI as a "Web" platform; in Certificates & secrets create a client secret. Issuer URL is `https://login.microsoftonline.com/<tenant-id>/v2.0`.
- **Google Workspace**: GCP Console → APIs & Services → Credentials → OAuth client ID (Web application). Issuer is `https://accounts.google.com`.
- **Authentik**: Applications → Providers → OAuth2/OpenID → SP-initiated. Issuer is `https://authentik.example.com/application/o/<slug>/`.

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

### 4. Test

SAML is **IdP-initiated** in v1. Users go to their IdP portal, click the
skill-pool application tile, and the IdP POSTs the signed assertion to our
ACS endpoint. The server:

1. Base64-decodes the `SAMLResponse` form field
2. Validates the XML signature against the stored IdP certificate (via
   `samael` → libxml2 + xmlsec1)
3. Checks `Conditions/NotOnOrAfter`
4. Pulls `email` from NameID or `email`/`mail`/`Email` attributes,
   `displayName` from `displayName`/`name`/(`givenName` + `surname`)
5. Upserts the user, ensures membership at `default_role`, mints a 14-day
   session, redirects to the web's `/saml-return?token=…&tenant=…`

SP-initiated SAML (generating `<AuthnRequest>`) is a follow-up — IdP-initiated
is what every modern IdP supports natively and what enterprise integrations
default to.

### Runtime dependencies (server host)

Signature validation needs xmlsec1 + libxml2. The server's Docker image
installs them; for non-Docker deploys:

- Debian/Ubuntu: `apt install libxml2 libxmlsec1 libxmlsec1-openssl`
- Nix: bundled in `flake.nix`'s dev shell and `nixosModules.skill-pool` (when wired)

### IdP-specific notes

- **Okta**: SAML 2.0 app integration. Set the Single Sign On URL = our ACS URL, Audience URI = our SP entity ID, NameID format = EmailAddress.
- **Azure AD / Entra**: Enterprise application → Set up single sign on → SAML. Identifier (Entity ID) = our SP entity ID, Reply URL = our ACS URL.
- **ADFS**: Add Relying Party Trust → import federation metadata from our metadata URL.

## Session lifetime

OIDC sessions last 14 days. There's no refresh-token rotation in this
scaffold — users re-OIDC on expiry. SAML sessions follow the same path
once the ACS handler is implemented.

API tokens (`spk_…`) minted via `admin token-create` are unaffected and
remain the canonical mechanism for the CLI.
