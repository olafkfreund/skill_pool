# Web UI (Phase 2 scaffold)

> Status: scaffold landed. Theme editor + real SSO + Monaco SKILL.md editor land in subsequent iterations of #4.

## Run locally

```bash
# 1. Start the API stack (Postgres + MinIO + server)
docker compose -f server/compose.yaml up -d postgres minio
sqlx migrate run --database-url postgres://skillpool:skillpool@localhost:5432/skillpool

# 2. Seed a tenant + token
cargo run --bin skill-pool-server -- admin tenant-create --slug acme --name Acme
cargo run --bin skill-pool-server -- admin token-create --tenant acme --name dev
# (note the printed spk_ token)

# 3. Start the API
cargo run --bin skill-pool-server &

# 4. Start the web dev server
cd web
npm install
SP_DEFAULT_TENANT=acme SKILL_POOL_API_BASE=http://127.0.0.1:8080 npm run dev
```

Open <http://localhost:5173/login>, paste the `spk_…` token, and you're in.

## Multi-tenant in dev

Three knobs decide which tenant the portal targets:

| Mechanism | When | Example |
|---|---|---|
| `Host` header subdomain | Production behind a wildcard cert | `acme.skill-pool.example.com` |
| `?tenant=` query string | Local dev without /etc/hosts edits | `http://localhost:5173/?tenant=acme` |
| `SP_DEFAULT_TENANT` env | Single-tenant deploy / dev fallback | `SP_DEFAULT_TENANT=acme npm run dev` |

The `hooks.server.ts` chain resolves in that order. See `src/lib/theme.ts` for the (currently hardcoded) per-tenant theme — the server-side `tenant_theme` table is wired into the editor in #9.

## Production build

```bash
cd web
npm run build
ORIGIN=https://your.host SKILL_POOL_API_BASE=https://api.your.host node build
```

`ORIGIN` must match the public URL — adapter-node uses it for CSRF
origin checks on form submissions.

## Theming

CSS custom properties drive everything:

```
--sp-primary --sp-primary-fg
--sp-accent
--sp-bg --sp-fg
--sp-muted --sp-muted-fg
--sp-border --sp-radius
```

`src/routes/+layout.svelte` injects them server-side into the page head
from the resolved theme. Free/Team tier customisation = setting these
values per tenant; Enterprise tier additionally allows a custom CSS
upload (#9).
