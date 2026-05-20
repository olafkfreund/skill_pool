# FAQ

> Real failure modes encountered during the first end-to-end build
> of skill-pool. If something breaks in a way that surprises you,
> chances are it surprised us too — start here.

## Build / dev environment

### Q: `cargo build` fails on `samael` with a libxml2 link error.

`samael` (the SAML signature validator) links against libxml2 and
xmlsec1 at build time. On NixOS you need both in your devshell
inputs:

```nix
devShells.default = pkgs.mkShell {
  buildInputs = with pkgs; [
    pkg-config
    libxml2
    libxmlsec       # or xmlsec_x with the openssl module
    openssl
    # ...
  ];
};
```

On Debian/Ubuntu: `apt install pkg-config libxml2-dev libxmlsec1-dev
libxmlsec1-openssl`.

The Docker image ships these — only non-container builds hit this.

### Q: Vite dev server returns 403 on a non-`localhost` hostname.

SvelteKit's Vite preview mode (and dev mode in some configs) rejects
hosts not in its allowlist. Symptom: you can hit `localhost:5173` but
`razer.lan:5173` returns 403 with "Blocked request".

Fix: add the hostname(s) to `vite.config.ts`:

```ts
export default defineConfig({
  // ...
  server: {
    host: '0.0.0.0',
    allowedHosts: ['localhost', '*.lan', '*.local', 'razer.lan', '*.nip.io'],
  },
});
```

A wildcard like `'*.lan'` is fine for dev. The PR fixing this in
this repo is commit `e13f8b1`.

### Q: SvelteKit returns 403 with "cross-site request" on form POSTs.

SvelteKit's CSRF protection rejects POSTs whose `Origin` header
doesn't match the configured allowed origins. Symptom: form actions
work in dev, fail in deploy.

Fix in `svelte.config.js`:

```js
const config = {
  kit: {
    csrf: {
      trustedOrigins: ['*'],  // or your specific hostnames
    },
  },
};
```

The PR fixing this in this repo is commit `142f501`. `'*'` is safe
because skill-pool also requires a bearer token on every POST — CSRF
on its own can't escalate.

### Q: `psql: could not connect to server: Connection refused`.

Two flavors:

1. **Host-side dev.** Postgres is on the host at port 5432 but the
   container/binary is looking for `127.0.0.1:55432` (or vice versa).
   The Docker Compose dev fixture maps host 55432 → container 5432.
   Pick one and stick with it:
   ```bash
   # Host-side binary against host Postgres:
   SKILL_POOL_DATABASE_URL=postgres://skillpool:skillpool@127.0.0.1:5432/skillpool

   # Host-side binary against the Compose-managed Postgres:
   SKILL_POOL_DATABASE_URL=postgres://skillpool:skillpool@127.0.0.1:55432/skillpool

   # In-container against same Compose-managed Postgres:
   SKILL_POOL_DATABASE_URL=postgres://skillpool:skillpool@postgres:5432/skillpool
   ```

2. **k8s deploy.** The `skill-pool-env` Secret is stale after a
   Terraform password rotation. Recreate:
   ```bash
   kubectl -n skill-pool delete secret skill-pool-env
   # Re-apply from your secret store (ESO will do this automatically).
   ```

## CLI

### Q: My `--config` flag seems to be ignored.

The clap arg is honored at parse time and the config loader reads
the file at that path. But a few subcommands re-resolve the config
later in their execution path and prefer
`$XDG_CONFIG_HOME/skill-pool/config.toml` or
`~/.skill-pool/config.toml`. Symptom: you point `--config` at a
non-default file but the subcommand still talks to the registry from
the default config.

Workaround: set `SKILL_POOL_CONFIG=/path/to/your.toml` as an env var.
That gets read consistently across every code path.

This is a known gap; tracked as a follow-up.

### Q: `skill-pool ensure` is silent on the happy path. Did it work?

Yes — `ensure` is intentionally silent when nothing changes (it's the
hot path of the `SessionStart` hook; chatty output every shell would
be noise). Add `RUST_LOG=skill_pool=debug` to see what it's doing.

`--quiet` further suppresses errors. Don't pass it from a terminal —
it's for the direnv hook only.

### Q: `skill-pool publish` 400s with "secret detected".

The server runs gitleaks rules on the bundle. False positives are
common with rotation runbooks, secret-handling skills, and any
SKILL.md that contains an *example* of a credential format.

Bypass: `--allow-secret` (or `allow_secret_scan = true` in the
tenant's config). Findings are still logged for audit but the
publish proceeds.

The capturer's pre-stage-2 + pre-POST scan has the same flag for the
same reason.

## Server

### Q: `/v1/healthz` returns `"status": "degraded"` but everything works.

Look at `deps.<name>.status`:

- `db: down` is real — that's a problem.
- `embedder: off` is normal — you didn't build with `--features
  fastembed`. The embedder is opt-in; the schema columns stay NULL,
  semantic search returns 400 ("not enabled on this server").
- `storage: down` is real — bundle downloads will fail.

The HTTP status is always 200 so the load balancer doesn't pull the
node on a transient blip. Page on `deps.db.status == "down"` from
your monitor, not on the HTTP code.

### Q: I get a tenant_resolution_failed even though my Host header looks right.

Three causes:

1. The leading label is `www` — the algorithm explicitly rejects
   that (subdomain routing is designed for tenant labels, not
   marketing pages).
2. The Host is a bare IPv4 or `localhost`. Set
   `X-Skill-Pool-Tenant: <slug>` for dev — the algorithm prefers the
   header when the Host doesn't yield a slug.
3. The tenant exists but is suspended. The server deliberately
   returns 401 for suspended/missing tenants — it does not leak the
   distinction between "no such slug" and "suspended slug" to
   unauthenticated callers.

### Q: My SSO login round-trips through the IdP but lands back at /login.

Check the OIDC `redirect_uri` registered with the IdP **exactly**
matches what the server is generating. Trailing slashes are
significant. The server logs the expected URI at `tracing=info`:

```text
INFO skill_pool::routes::auth::oidc: redirecting to IdP
  redirect_uri="https://acme.skill-pool.example.com/v1/auth/oidc/acme/callback"
```

Copy that exact string into your IdP's config.

## Web portal

### Q: The login page is unstyled / "loading…" forever.

The portal's request-time theme resolver tries to call `GET /v1/theme`
on the API. If the API base URL the SvelteKit container thinks is
correct is unreachable, the page falls back to default branding —
but if the call hangs (vs erroring fast), you can see a brief
"loading" state.

Fix: check `PUBLIC_API_BASE_URL` (or whatever the deploy sets) is
reachable from inside the SvelteKit pod, not just from your laptop.
Inside k8s, that's usually `http://server:8080` (service DNS), not
the public hostname.

### Q: `$lib/server/...` imports show up in the client bundle.

You aliased a server-only module from a `+page.svelte` instead of a
`+page.server.ts`. SvelteKit's `$lib/server` namespace is a hard
boundary — modules under it can be imported only from server code
(load functions, hooks, form actions).

Fix: move the import into a `+page.server.ts` load function and pass
the result to the page via `data`. The server module never ships to
the browser.

## README and wiki

### Q: My README's WebM demo doesn't render on the wiki.

GitHub-flavored Markdown supports inline video via the `<video>`
tag, but GitHub's renderer falls back to a static frame in many
contexts (RSS feeds, the wiki). Provide a GIF fallback or link to
the WebM:

```html
<video src="docs/demo.webm" controls width="800">
  Your browser doesn't support inline video.
  <a href="docs/demo.webm">Download demo.webm</a>.
</video>
```

### Q: My `[[wiki-link]]` syntax doesn't work.

GitHub wikis don't use `[[wiki-link]]` (that's MediaWiki / DokuWiki
syntax). Use plain Markdown:

```markdown
See [Architecture](Architecture.md) for details.
```

GitHub auto-resolves bare page names to wiki pages within the same
repo.

### Q: My `_Sidebar.md` doesn't render as the sidebar.

It does — but only on pages **other than** the one you're editing. The
sidebar renders on actual viewable wiki pages, not in the editor
preview. Save and navigate to any page to see it.

Page name must be exactly `_Sidebar.md` (capital S, leading
underscore).

## Where to read next

- [Operator Guide](Operator-Guide.md) — every deploy path
- [Tenant Onboarding](Tenant-Onboarding.md) — first-tenant playbook
- [Decisions Log](Decisions-Log.md) — why some of these gotchas exist

## Cross-links into the codebase

- `web/vite.config.ts` — `allowedHosts` array
- `web/svelte.config.js` — `kit.csrf.trustedOrigins`
- `cli/src/config.rs` — config loader (where the `--config` gap lives)
- `server/src/secret_scan.rs` — gitleaks rules
- `server/src/tenant.rs::TenantCtx::from_request_parts` — resolution
  algorithm
