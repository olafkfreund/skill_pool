# Tenant Onboarding

> Step-by-step playbook for an operator standing up a new tenant on a
> running skill-pool deploy. End-to-end: tenant created → first token
> minted → SSO wired → tenant-scoped catalog populated → developer
> installed. ~30 minutes if you have all the IdP credentials ready.

This page assumes the server is already running. If you don't have one
yet, start at [Operator Guide](Operator-Guide.md) — pick a deploy path,
verify `/v1/healthz`, then come back here.

## Pre-flight

- `skill-pool-server` binary in your `$PATH` (or accessible via
  `kubectl exec deploy/skill-pool-server -- skill-pool-server ...` for
  Helm deploys).
- `SKILL_POOL_DATABASE_URL` set to the same Postgres the server uses.
- A wildcard DNS record `*.skill-pool.example.com` pointing at the
  reverse proxy (Caddy / ALB / Traefik). The single-node and AWS
  deploy guides walk through this; if you're using `nip.io` you can
  skip DNS entirely.
- (Optional) Your IdP's OIDC issuer URL + client ID + client secret,
  or SAML signing certificate. See [SSO Setup](SSO-Setup.md) if you don't
  have them yet.

## Step 1 — Create the tenant

```bash
skill-pool-server admin tenant-create \
  --slug acme \
  --name "Acme Inc." \
  --plan team
```

`--slug` is the leftmost label in subdomain routing
(`acme.skill-pool.example.com`). Lowercase, kebab-case, ≤ 32 chars.
The command is idempotent — re-running with the same slug returns the
existing row.

`--plan` is one of `free`, `team`, `enterprise`. It gates feature
flags (custom domains and dedicated mode are enterprise-only) but
doesn't enforce billing — that's out of scope.

Verify:

```bash
skill-pool-server admin tenant-list
# slug   name        plan   status   created_at
# acme   Acme Inc.   team   active   2026-05-20T08:39:04Z
```

## Step 2 — Mint a bootstrap token

The bootstrap token is what you'll use to publish the first batch of
skills, configure SSO, and (later) hand to your CI.

```bash
skill-pool-server admin token-create \
  --tenant acme \
  --name bootstrap \
  --scope "skills:publish skills:read tenant:admin"

# token created
#   id:     1ec92531-1942-41b3-ab3d-57763377d5c6
#   tenant: acme
#   scope:  skills:publish skills:read tenant:admin
#
# RAW TOKEN (shown once — copy now):
#   spk_f00ae6c8ceddad0095b9edc413f4de1c35a781b5aa45d4f201d86908897ca2ca
```

The raw token is printed **once** — the DB stores only its SHA-256
hash. Save it in your password manager.

Confirm it works:

```bash
curl -s \
  -H "Authorization: Bearer spk_…" \
  -H "X-Skill-Pool-Tenant: acme" \
  http://127.0.0.1:8080/v1/skills | jq 'length'
# 0
```

(Zero skills until you publish some — see Step 4.)

## Step 3 — Wire SSO (optional, but do this before users sign in)

If you'll have humans logging into the portal (most teams), set up
SSO before they hit the login page. The first user to sign in via SSO
becomes the tenant admin (`default_role = admin` for the first user;
the configured `default_role` for everyone else).

### OIDC (Okta, Azure AD, Google Workspace, Authentik)

1. In your IdP, register skill-pool as an OIDC client. Redirect URI:
   ```
   https://acme.skill-pool.example.com/v1/auth/oidc/acme/callback
   ```
   Scopes: `openid email profile`. Grant types: Authorization Code.
   PKCE required.

2. Capture the issuer URL, client ID, and client secret.

3. Configure the tenant:
   ```bash
   skill-pool-server admin sso-set \
     --tenant acme \
     --issuer 'https://acme.okta.com/oauth2/default' \
     --client-id 'YOUR_CLIENT_ID' \
     --client-secret 'YOUR_CLIENT_SECRET' \
     --default-role publisher
   ```

4. Visit `https://acme.skill-pool.example.com/login` — you should see
   "Sign in with SSO (OIDC)". Click it; round-trip through the IdP;
   land in the catalog.

Full per-IdP notes (Okta tile shapes, Azure AD app-registration
gotchas, Google Workspace's group-claim caveat) in [SSO Setup](SSO-Setup.md).

### SAML 2.0 (ADFS, on-prem Shibboleth, legacy Okta SAML)

1. Hand the IdP admin your SP metadata URL:
   ```
   https://acme.skill-pool.example.com/v1/auth/saml/acme/metadata
   ```

2. From the IdP, capture the entity ID, SSO URL, and X.509 signing
   certificate (PEM).

3. Configure the tenant:
   ```bash
   skill-pool-server admin saml-set \
     --tenant acme \
     --idp-entity-id 'https://acme.okta.com/exk…' \
     --idp-sso-url 'https://acme.okta.com/app/.../sso/saml' \
     --idp-cert-path /path/to/idp.pem \
     --default-role publisher
   ```

4. Users click the skill-pool tile in their IdP portal. IdP-initiated
   only in v1.

## Step 4 — Configure stack mappings (optional)

The `skill-pool bootstrap` command on the developer side asks the
server "what skills do you recommend for a Rust + Postgres project?"
The mapping lives in the catalog itself — `tags` on skills are
intersected with the project's detected stack.

For best `bootstrap` results, tag your team's skills with stack
labels at publish time:

```yaml
---
name: axum-handler
description: …
tags: [rust, axum, postgres]
---
```

The catalog already surfaces `rust`, `python`, `typescript`, etc. as
tags on bundled skills (see [Bundled Skills](Bundled-Skills.md)); your
internal additions extend the same set.

## Step 5 — Invite users

If SSO is wired, users sign in via the IdP portal — no invite
required. The group-mapping section of [SSO Setup](SSO-Setup.md) shows
how to map IdP groups to skill-pool roles (`viewer` < `publisher` <
`curator` < `admin`).

If you're running without SSO (small team, single tenant), use the
admin CLI to add users + mint personal tokens:

```bash
# Add a user row (uses email as identity; no password).
skill-pool-server admin user-create \
  --tenant acme \
  --email alice@example.com \
  --role publisher

# Personal token for Alice's CLI.
skill-pool-server admin token-create \
  --tenant acme \
  --name "alice-cli" \
  --scope "skills:read skills:publish"
# Hand the raw token to Alice via your usual secrets channel.
```

## Step 6 — First publish

From any developer machine that has the CLI installed:

```bash
mkdir hello-skill
cat > hello-skill/SKILL.md <<'MD'
---
name: hello-skill
description: A trivial smoke-test skill.
tags: [test]
---

# hello-skill

Smoke-test content.
MD

skill-pool login --registry https://acme.skill-pool.example.com --tenant acme
# Paste bootstrap token

skill-pool publish ./hello-skill --version 0.1.0
# ✓ hello-skill@0.1.0 published
```

Verify:

```bash
skill-pool search
# slug          version  description                   tags
# hello-skill   0.1.0    A trivial smoke-test skill.   test
```

## Step 7 — Developer install

On a fresh developer machine, after `skill-pool login`:

```bash
cd ~/projects/my-app
skill-pool init                       # writes .skill-pool/manifest.toml
skill-pool add hello-skill            # adds + installs
skill-pool hook-install --with-scorer # SessionStart + Stop + SessionEnd hooks
```

Next `claude-code` session loads `hello-skill` automatically via the
`SessionStart` hook.

## Step 8 — Theming (optional)

Open the admin theme page in your browser at
`https://acme.skill-pool.example.com/admin/theme`. Upload a logo, pick
a palette, save. The WCAG AA contrast check gates the save — see
[Theming](Theming.md) for the full surface.

## Step 9 — Capturer (optional, recommended)

For developers who want the LLM capturer to draft skills from their
sessions automatically:

```bash
# One-time, per dev machine.
skill-pool hook-install --with-scorer  # installs Stop + SessionEnd hooks

# Pick a mode (mutually exclusive — see Phase-4-Capture for the trade-off):
# Mode A — hourly timer
cp packaging/systemd/skill-pool-capturer.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now skill-pool-capturer.timer

# Mode B — long-lived daemon
cp packaging/systemd/skill-pool-capturer-daemon.service ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now skill-pool-capturer-daemon.service
```

The daemon requires `ANTHROPIC_API_KEY` in its environment (Stage 1
Haiku → Stage 2 Sonnet → POST `/v1/drafts`). Full detail in
[Phase 4 — Capture](Phase-4-Capture.md).

## Step 10 — Custom domain (Enterprise, optional)

If the tenant wants `skills.acme.com` instead of
`acme.skill-pool.example.com`:

```bash
# 1. Claim the hostname (returns a TXT record to paste into DNS).
skill-pool-server admin custom-domain --tenant acme \
  add --hostname skills.acme.com

# 2. Tenant pastes the TXT record into their DNS provider.

# 3. Verify (runs upstream TXT lookup).
skill-pool-server admin custom-domain --tenant acme \
  verify --id <UUID>

# 4. Tenant CNAMEs skills.acme.com to your registry's reverse proxy.

# 5. First request triggers on-demand TLS issuance.
```

Caddy / Traefik on-demand-TLS hook integration in
[Custom-Domain-ACME](Custom-Domain-ACME.md).

## Quick rollback if anything goes sideways

- **SSO sign-in broken** — `skill-pool-server admin sso-clear --tenant
  acme` reverts to email-only login.
- **Bad theme saved** — admin theme page → "Reset to default", or via
  the API: `PUT /v1/theme` with the default palette.
- **Bad publish** — `POST /v1/skills/{slug}/archive` (admin scope).
- **Tenant fully broken** — `skill-pool-server admin tenant-suspend
  --tenant acme` blocks all sign-ins and API calls; un-suspend later
  with the same verb minus `suspend`.

## Where to read next

- [SSO Setup](SSO-Setup.md) — full per-IdP walkthrough
- [Theming](Theming.md) — palette, logo, favicon, custom CSS
- [Custom Domain + ACME](Custom-Domain-ACME.md) — `skills.acme.com`
- [Phase 4 — Capture](Phase-4-Capture.md) — capturer daemon detail
- [CLI Reference](CLI-Reference.md) — every developer-side subcommand
- [FAQ](FAQ.md) — real failure modes from the first install

## Cross-links into the codebase

- `server/src/admin.rs` — every admin subcommand
- `server/src/routes/auth/oidc.rs` — OIDC callback
- `server/src/routes/auth/saml.rs` — SAML ACS handler
- `server/src/routes/tenants.rs` — tenant CRUD via the API
- `packaging/systemd/` — capturer unit files
