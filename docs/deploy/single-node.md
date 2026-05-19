# Single-node deploy (systemd + Postgres + Caddy)

The "one box on a Pi, in a homelab, or a small VPS" path. ~5 minutes of
configuration. Suitable up to a few dozen developers on a single tenant
(or a few small tenants).

## What you'll end up with

```
┌──────────────┐  TLS    ┌──────────────┐
│  internet    │ ─────►  │   Caddy      │
└──────────────┘         │ :80/:443     │
                         └──────┬───────┘
                                │ HTTP
        ┌───────────────────────┼────────────────────────┐
        │                       │                        │
        ▼                       ▼                        ▼
  ┌────────────┐         ┌────────────┐          ┌───────────────┐
  │ skill-pool │   sqlx  │  Postgres  │  files   │  /var/lib/    │
  │  server    │ ──────► │            │ ───────► │  skill-pool/  │
  │  :8080     │         │  :5432     │          │  bundles/     │
  └────────────┘         └────────────┘          └───────────────┘
        ▲
        │ HTTP (SSR)
  ┌─────┴──────┐
  │  skill-pool│
  │    web     │
  │   :3000    │
  └────────────┘
```

## Steps

### 1. Postgres

```bash
sudo apt install -y postgresql-16
sudo -u postgres psql <<'SQL'
  CREATE ROLE skillpool LOGIN PASSWORD 'changeme';
  CREATE DATABASE skillpool OWNER skillpool;
  \c skillpool
  CREATE EXTENSION IF NOT EXISTS vector;
SQL
```

Run migrations against the new database. The server binary itself does
**not** auto-migrate on startup — migrations are a separate step so a
broken deploy can never run a migration as a side effect:

```bash
sqlx migrate run --source server/migrations \
  --database-url 'postgres://skillpool:changeme@localhost/skillpool'
```

### 2. Install the binary

```bash
# Either from the pre-built Docker image, or build from source:
cargo build --release -p skill-pool-server
sudo install -o root -g root -m 0755 \
  target/release/skill-pool-server /usr/local/bin/
```

### 3. systemd unit

```bash
sudo useradd --system --home /var/lib/skill-pool --shell /usr/sbin/nologin skillpool
sudo mkdir -p /var/lib/skill-pool/bundles /etc/skill-pool
sudo chown -R skillpool:skillpool /var/lib/skill-pool

sudo cp packaging/systemd/skill-pool-server.service /etc/systemd/system/
sudo install -o skillpool -g skillpool -m 0600 \
  packaging/systemd/skill-pool-server.env.example \
  /etc/skill-pool/skill-pool-server.env

sudoedit /etc/skill-pool/skill-pool-server.env   # paste real DSN + secrets

sudo systemctl daemon-reload
sudo systemctl enable --now skill-pool-server
journalctl -u skill-pool-server -f
```

### 4. Caddy in front

```bash
sudo cp packaging/proxy/Caddyfile /etc/caddy/Caddyfile
sudoedit /etc/caddy/Caddyfile   # set your real domain + email
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

For wildcard certs (tenant subdomains), add a DNS provider plugin and
uncomment the `acme_dns` line in the Caddyfile.

### 5. Create your first tenant

```bash
sudo -u skillpool skill-pool-server admin tenant-create \
  --slug acme --name "Acme Inc."
sudo -u skillpool skill-pool-server admin token-create \
  --tenant acme --name bootstrap
# → prints the raw token once. Save it; use as `Authorization: Bearer …`.
```

## Backup

Two things to back up:

1. **Postgres**: `pg_dump skillpool` nightly. The DB carries metadata,
   tenants, tokens, themes, audit log.
2. **Bundle storage**: tar `/var/lib/skill-pool/bundles` weekly (changes
   slowly; bundles are immutable once published).

For object-storage backends (S3/GCS), enable bucket versioning instead.

The deploy / rollback workflow that uses these backups is documented
in `docs/ops/rollback.md` (forward-only sqlx migrations + restore from
snapshot — read it before your first production deploy).

## Where to go from here

- Need read replicas? See `docs/deploy/kubernetes.md` — same image, just
  set `SKILL_POOL_DATABASE_READ_URL` and the server routes reads.
- Want declarative config? See `docs/deploy/nixos.md`.
