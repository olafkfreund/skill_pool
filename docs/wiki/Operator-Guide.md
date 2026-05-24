# Operator Guide

> Every supported deploy path collated. Pick one based on your existing
> ops surface; the same Rust binary, same migrations, same backup
> rules apply across all of them. Estimated time-to-first-tenant: 5 min
> (single-node) to 30 min (AWS EKS).

## Deploy path picker

| Path                              | When                                                          |
|-----------------------------------|---------------------------------------------------------------|
| **Single-node systemd + Caddy**   | Homelab, small VPS, ~dozen developers, one or two tenants     |
| **NixOS module**                  | You already manage NixOS hosts declaratively                  |
| **Docker / Docker Compose**       | You want a one-host containerized deploy without k8s overhead |
| **Kubernetes (Helm)**             | You have a cluster already and want a stock chart             |
| **AWS EKS (Terraform)**           | You're starting from zero and want a turnkey AWS deploy       |

Each path is detailed below. All of them produce the same observable
behavior — same routes, same metrics endpoint at `/metrics`, same
healthcheck at `/v1/healthz`.

## Pre-flight (every path)

You need:

- **Postgres 16+** with the `pgvector` extension. The default builds
  work without `pgvector`; `--features fastembed` requires it.
- **A directory or bucket** for bundle storage. Any opendal-supported
  backend works: `fs://`, `s3://`, `gcs://`, `azblob://`.
- **A reverse proxy** that can do TLS termination and route the
  `/v1/*` + `/metrics` paths to the server, everything else to the
  SvelteKit portal. Caddy, Traefik, nginx, and the AWS ALB all work.
- (Optional) **Redis** — used as a read-through cache, rate-limit
  store, and job queue. The server falls back gracefully when it's
  absent (caches become no-ops, rate limits fail-open, jobs run
  inline).

## Path 1 — Single-node systemd + Caddy

Reference: `docs/deploy/single-node.md`.

### 1. Postgres

```bash
sudo apt install -y postgresql-16
sudo -u postgres psql <<'SQL'
  CREATE ROLE skillpool LOGIN PASSWORD 'changeme';
  CREATE DATABASE skillpool OWNER skillpool;
  \c skillpool
  CREATE EXTENSION IF NOT EXISTS vector;
SQL

sqlx migrate run --source server/migrations \
  --database-url 'postgres://skillpool:changeme@localhost/skillpool'
```

The server does **not** auto-migrate on startup — migrations are a
separate step so a broken deploy can't run a migration as a side
effect.

### 2. Binary + systemd

```bash
cargo build --release -p skill-pool-server
sudo install -o root -g root -m 0755 \
  target/release/skill-pool-server /usr/local/bin/

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

### 3. Caddy

```bash
sudo cp packaging/proxy/Caddyfile /etc/caddy/Caddyfile
sudoedit /etc/caddy/Caddyfile   # set your real domain + email
sudo caddy validate --config /etc/caddy/Caddyfile
sudo systemctl reload caddy
```

Wildcard certs (tenant subdomains) need a DNS provider plugin —
uncomment the `acme_dns` line in the shipped Caddyfile.

### 4. First tenant

```bash
sudo -u skillpool skill-pool-server admin tenant-create \
  --slug acme --name "Acme Inc."
sudo -u skillpool skill-pool-server admin token-create \
  --tenant acme --name bootstrap
```

See [Tenant Onboarding](Tenant-Onboarding.md) for the rest of the
first-tenant playbook.

---

## Path 2 — NixOS module

Reference: `docs/deploy/nixos.md`.

### Flake input

```nix
{
  inputs.skill-pool.url = "github:olafkfreund/skill_pool";
  outputs = { self, nixpkgs, skill-pool, ... }: {
    nixosConfigurations.registry = nixpkgs.lib.nixosSystem {
      system = "x86_64-linux";
      modules = [
        skill-pool.nixosModules.skill-pool-server
        ./registry-config.nix
      ];
    };
  };
}
```

### Minimal configuration

```nix
{ pkgs, skill-pool, ... }:
{
  services.skill-pool-server = {
    enable = true;
    package = skill-pool.packages.${pkgs.system}.skill-pool-server;

    bind = "127.0.0.1:8080";
    storageUri = "fs:///var/lib/skill-pool/bundles";
    defaultTenant = "acme";

    environmentFile = "/run/keys/skill-pool.env";
  };

  services.postgresql = {
    enable = true;
    package = pkgs.postgresql_17;
    ensureDatabases = [ "skillpool" ];
    ensureUsers = [{ name = "skillpool"; ensureDBOwnership = true; }];
    extraPlugins = ps: [ ps.pgvector ];
  };

  services.caddy = {
    enable = true;
    virtualHosts."skill-pool.example.com".extraConfig = ''
      reverse_proxy 127.0.0.1:3000
    '';
    virtualHosts."*.skill-pool.example.com".extraConfig = ''
      @api path /v1/* /metrics
      reverse_proxy @api 127.0.0.1:8080
      reverse_proxy 127.0.0.1:3000
    '';
  };
}
```

### Secrets with agenix

```nix
age.secrets."skill-pool.env" = {
  file = ./secrets/skill-pool.env.age;
  owner = config.services.skill-pool-server.user;
  group = config.services.skill-pool-server.group;
  mode = "0400";
};

services.skill-pool-server.environmentFile =
  config.age.secrets."skill-pool.env".path;
```

### Module options

| Option            | Type            | Default                              |
|-------------------|-----------------|--------------------------------------|
| `enable`          | bool            | `false`                              |
| `package`         | package         | —                                    |
| `bind`            | string          | `"127.0.0.1:8080"`                   |
| `databaseUrl`     | nullable string | `null`                               |
| `storageUri`      | string          | `"fs:///var/lib/skill-pool/bundles"` |
| `defaultTenant`   | nullable string | `null`                               |
| `logLevel`        | string          | `"info,skill_pool=info"`             |
| `logFormat`       | enum            | `"json"`                             |
| `otlpEndpoint`    | nullable string | `null`                               |
| `environmentFile` | nullable path   | `null`                               |
| `user` / `group`  | string          | `"skillpool"`                        |
| `stateDir`        | path            | `/var/lib/skill-pool`                |
| `openFirewall`    | bool            | `false`                              |

### Web bundle

The flake exposes `packages.skill-pool-web`, a `buildNpmPackage`
derivation that produces the adapter-node SvelteKit bundle. Use:

```bash
nix build .#skill-pool-web && PORT=3000 node result/index.js
```

### Rebuild + verify

```bash
sudo nixos-rebuild switch --flake .#registry
systemctl status skill-pool-server
journalctl -u skill-pool-server -f
curl -s http://127.0.0.1:8080/v1/healthz | jq
```

---

## Path 3 — Docker / Docker Compose

The repo ships two Dockerfiles (`server/Dockerfile`, `web/Dockerfile`).
A minimal Compose looks like:

```yaml
version: "3.9"
services:
  postgres:
    image: postgres:16
    environment:
      POSTGRES_DB: skillpool
      POSTGRES_USER: skillpool
      POSTGRES_PASSWORD: changeme
    volumes: ["pgdata:/var/lib/postgresql/data"]

  server:
    image: ghcr.io/olafkfreund/skill-pool-server:v0.1.0
    environment:
      SKILL_POOL_DATABASE_URL: postgres://skillpool:changeme@postgres/skillpool
      SKILL_POOL_STORAGE_URI: fs:///var/lib/skill-pool/bundles
    volumes: ["bundles:/var/lib/skill-pool/bundles"]
    depends_on: [postgres]

  web:
    image: ghcr.io/olafkfreund/skill-pool-web:v0.1.0
    environment:
      PUBLIC_API_BASE_URL: http://server:8080
      ORIGIN: https://skill-pool.example.com
    depends_on: [server]

  caddy:
    image: caddy:2
    ports: ["80:80", "443:443"]
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile
      - caddy_data:/data

volumes: { pgdata: {}, bundles: {}, caddy_data: {} }
```

Migrations run as a one-shot:

```bash
docker compose run --rm server skill-pool-server migrate
```

---

## Path 4 — Kubernetes (Helm)

Reference: `docs/deploy/kubernetes.md` + `deploy/helm/skill-pool/`.

```bash
# 1. Ensure the namespace + Secret exist.
kubectl create namespace skill-pool
kubectl -n skill-pool create secret generic skill-pool-env \
  --from-literal=SKILL_POOL_DATABASE_URL='postgres://…' \
  --from-literal=SKILL_POOL_EMAIL_SECRET_KEY="$(openssl rand -hex 32)"

# 2. Run migrations as a one-shot Job.
kubectl -n skill-pool run sqlx-migrate \
  --rm -it --restart=Never \
  --image ghcr.io/olafkfreund/skill-pool-server:v0.1.0 \
  --env "SKILL_POOL_DATABASE_URL=…" \
  --command -- /usr/local/bin/skill-pool-server migrate

# 3. Install the chart.
helm install skill-pool ./deploy/helm/skill-pool \
  -f deploy/helm/skill-pool/values.yaml \
  -n skill-pool
```

`values.yaml` keys you care about:

- `image.server.tag` / `image.web.tag` — pin specific versions.
- `server.env.SKILL_POOL_STORAGE_URI` — S3/GCS/Azure bucket URI.
- `ingress.hosts[].host` — the public hostname.
- `ingress.annotations` — cert-manager issuer, ALB attributes, etc.
- `redis.existingSecret` — if you bring Redis, name of a Secret with
  `SKILL_POOL_REDIS_URL`.

Pre-upgrade Helm hook handles migrations automatically on every
`helm upgrade`. To roll back: `helm rollback skill-pool <REV>`. The
old binary reads the new schema fine because all schema changes are
additive (see `docs/ops/rollback.md`).

---

## Path 5 — AWS EKS (Terraform)

Reference: `docs/deploy/aws.md` + `deploy/terraform/aws/`.

The Terraform starter provisions:

- A VPC across 2 AZs (or 3 if you want HA).
- An EKS cluster with managed node groups.
- An RDS Postgres 16 instance with `pgvector` preloaded.
- An S3 bucket for bundles with versioning enabled.
- ECR repos for both images.
- An IAM role for IRSA so the pod can write to S3 without keys.
- A GitHub OIDC provider + an IAM role with permissions for the
  build/deploy workflows.
- The AWS Load Balancer Controller via Helm.
- cert-manager + a Let's Encrypt cluster issuer.

End-to-end:

```bash
cd deploy/terraform/aws/
${EDITOR:-vim} variables.tf   # region, azs, github_repository
terraform init && terraform apply

# ~20 min later, connect to the cluster:
aws eks update-kubeconfig --region "$(terraform output -raw region)" \
                          --name   "$(terraform output -raw cluster_name)"

# Bridge Secrets Manager → k8s Secret (or use External Secrets Operator).
# Then run migrations and helm install — same as Path 4.

helm install skill-pool ./deploy/helm/skill-pool \
  -f deploy/helm/skill-pool/values-aws.yaml \
  -n skill-pool --create-namespace
```

### TLS — `nip.io` + Let's Encrypt (no domain required)

The default deploy uses `<dashed-ip>.nip.io` for DNS — no domain
purchase needed. cert-manager + the LE HTTP-01 challenge issues the
cert.

```bash
ALB_HOST=$(kubectl -n skill-pool get ingress skill-pool \
  -o jsonpath='{.status.loadBalancer.ingress[0].hostname}')
ALB_IP=$(dig +short "$ALB_HOST" | head -n1)
DASHED_IP="${ALB_IP//./-}"

helm upgrade skill-pool ./deploy/helm/skill-pool \
  -f deploy/helm/skill-pool/values-aws.yaml \
  --set ingress.hosts[0].host="skill-pool.${DASHED_IP}.nip.io" \
  --reuse-values
```

Wait 30–120s for cert issuance:

```bash
kubectl -n skill-pool get certificate -w
```

### Cost (lean baseline, eu-west-1, May 2026)

| Component        | Monthly  |
|------------------|----------|
| EKS control plane | $73     |
| 2× t3.medium     | $60      |
| RDS t4g.medium   | $50      |
| ALB              | $22      |
| NAT (single AZ)  | $32      |
| Misc (S3, ECR, R53, SM) | $11 |
| **Total**        | **~$248**|

HA (Multi-AZ RDS, per-AZ NAT, third worker): +$110/mo. Dev/staging
(single Spot, no NAT): ~$130/mo.

---

## GitHub Actions CI/CD

Four workflows ship in `.github/workflows/`:

| Workflow             | Triggers                          | Purpose                                                |
|----------------------|-----------------------------------|--------------------------------------------------------|
| **CI**               | push to `main`, PRs               | fmt + clippy + tests + web lint + helm lint            |
| **Build & push**     | push to `main`, tag `v*`, manual  | Build + push both images to ECR                        |
| **Deploy to EKS**    | tag `v*`, manual                  | `helm upgrade` + smoke-test + auto-rollback on failure |
| **DB migrations**    | manual only                       | Break-glass: run `sqlx migrate run` from a one-shot pod |

All AWS-touching workflows authenticate via **OIDC** — no long-lived
AWS keys in GitHub. The only repo-level secret is `AWS_ROLE_ARN`.

Required repo-level **variables**:

| Name | Example |
|---|---|
| `AWS_REGION` | `eu-west-1` |
| `ECR_REPO_SERVER` | `skill-pool/server` |
| `ECR_REPO_WEB` | `skill-pool/web` |
| `EKS_CLUSTER_NAME` | `skill-pool-prod` |
| `HELM_RELEASE_NAME` | `skill-pool` |
| `HELM_NAMESPACE` | `skill-pool` |
| `PUBLIC_HOSTNAME` | `skill-pool.example.com` |

Image tagging: pushes to `main` tag `<git-sha>` + `latest`; pushes of
`v*` tag also push `<git-ref-name>` (semver). `values-aws.yaml` pins
specific tags — `latest` is for human convenience only and should
never be referenced by the cluster.

Auto-rollback: `deploy.yml` runs `helm rollback` if the rollout-status
or smoke-test step fails after a successful `helm upgrade`.

---

## Backup & restore

### Postgres

```bash
# Single node — daily cron:
pg_dump -Fc skillpool > /backups/skillpool-$(date +%F).dump

# RDS — automated snapshots; bump retention to 30 days for prod.
```

### Bundle storage

- **fs://** — tar `/var/lib/skill-pool/bundles` weekly. Bundles are
  immutable once published, so the diff is small.
- **s3://** — turn on bucket versioning. Lifecycle rule: delete
  non-current versions after 90 days.

### Restore drill

Recommended every quarter:

1. `pg_restore` into a scratch Postgres.
2. Point a scratch `skill-pool-server` at it with `--storage-uri`
   pointed at the bundle-storage backup.
3. `curl /v1/healthz`, list skills, download one.

Full rollback procedures: `docs/ops/rollback.md`.

---

## Day-2 ops

- **Metrics** — `/metrics` on the server in Prometheus format. The
  Grafana dashboard ships in `ops/grafana/skill-pool.json`; the
  Prometheus alert rules in `ops/prometheus/skill-pool.rules.yaml`.
- **Tracing** — set `SKILL_POOL_OTLP_ENDPOINT=http://collector:4317`.
  All request/response spans plus the background-task spans go out
  with `service.name=skill-pool-server`.
- **Logs** — JSON to stdout by default (`SKILL_POOL_LOG_FORMAT=json`).
  Pretty mode for dev: `SKILL_POOL_LOG_FORMAT=pretty`.
- **Runbook** — `docs/ops/runbook.md` covers the SLO breach playbook
  per top-N alert.
- **Capacity planning** — `docs/ops/capacity.md` covers the tier-by-tier
  sizing curve.
- **Rollback** — `docs/ops/rollback.md` covers the forward-only
  migration discipline + DR from snapshots.

---

## Plugin storage

Plugins (per-tenant Claude Code marketplace, see
[`docs/plugins.md`](../plugins.md)) introduce two operator-visible
concerns beyond skill bundles: on-disk bare git repos for the
`/git/plugins/<slug>.git` endpoint, and the per-tenant pre-rendered
marketplace cache in Postgres.

### On-disk layout

Source of truth: `server/src/storage.rs:71-94`
(`Storage::plugin_git_path`).

For each `internal`- or `mirror`-sourced plugin, skill-pool
materialises a bare git repo on first publish at:

```text
<storage-root>/<tenant-uuid>/plugins/<slug>.git/
```

`<storage-root>` is the path component of `SKILL_POOL_STORAGE_URI`
when it starts with `fs://`. The git endpoint **requires `fs://`
storage** — S3/GCS/Azure backends cannot serve git-upload-pack and
the endpoint returns an explicit error at publish time. A per-process
checkout cache for object-store-backed plugin git is deferred.

The tenant UUID prefix is the same one bundle storage uses, so
`rm -rf <storage-root>/<tenant-uuid>/` cleans plugin repos and skill
bundles in one shot when a tenant is decommissioned.

### Backup

Bare git repos are append-mostly trees of immutable blobs (a publish
only adds objects; archive flips a DB row, never deletes files). Two
practical rules:

1. **Include `<storage-root>/<tenant-uuid>/plugins/` in the same
   backup job that snapshots `bundles/`.** A daily `tar` of
   `<storage-root>` covers both. Incremental backup tools (`restic`,
   `borg`) deduplicate well — only newly published plugin objects
   transfer on each run.
2. **A restored repo serves correctly without re-materialisation
   from Postgres.** Trees + blobs are self-contained; the only DB
   row needed is `plugins` (for the `sourcing_mode` check in
   `plugin_git::resolve_repo_path`).

If the bare repo is missing after a publish (storage write failed
silently — logged at warn level), the API returns 404 from
`/git/plugins/<slug>.git`. **Recovery: republish.** The materialiser
is idempotent — the second pass walks the same content tree and
writes the same objects.

### Marketplace cache

Source of truth: `server/migrations/0032_plugin_marketplace_entries.sql`.

The `plugin_marketplace_entries` table holds one row per
`(tenant_id, plugin_slug)` — the latest published version pre-rendered
into the exact JSON object that splices into
`/.claude-plugin/marketplace.json`. The marketplace handler
(`server/src/routes/marketplace.rs`) is a single SELECT plus a JSON
wrapper, with a strong ETag and `Cache-Control: public, max-age=60`
on the response. Conditional GETs return 304 on match.

Storage cost: a few hundred bytes per plugin per tenant — negligible
versus the bundle tarballs.

### Mirror refresh (deferred)

`mirror`-sourced plugins are listed in `marketplace.json` and have a
local bare repo row, but the periodic-pull worker that refreshes
those repos from upstream is tracked in a follow-up issue. Until it
ships:

- Mirror plugins serve whatever the upstream tree looked like at
  publish time (or when the operator manually re-runs a publish).
- A stale mirror does not break clones — the cached objects still
  serve. It just doesn't pick up upstream commits automatically.

If you need fresh mirror content before the worker lands, republish
the plugin (which re-materialises the tree from upstream).

### Growth notes

- Plugin bare repos grow with `commits × tree-size`. A plugin
  bundling a dozen skills with one publish per week settles at
  single-digit MB after a year.
- The 256 KiB cap on the publish-time `manifest` body
  (`server/src/routes/plugins.rs:42`) caps the inline-blob blast
  radius — operators don't need a separate per-plugin size monitor.
- The total number of plugins per tenant has no hard cap, but the
  marketplace JSON is fetched on every Claude Code refresh; tenants
  with thousands of plugins will see noticeable cold-fetch latency.
  Tier-by-tier sizing for plugins-heavy tenants is on the capacity
  planning backlog (`docs/ops/capacity.md`).

---

## Where to read next

- [Tenant Onboarding](Tenant-Onboarding.md) — first-tenant playbook
- [SSO Setup](SSO-Setup.md) — OIDC + SAML per IdP
- [Custom Domain + ACME](Custom-Domain-ACME.md) — per-tenant hostnames
- [API Reference](API-Reference.md) — every endpoint
- [FAQ](FAQ.md) — real failure modes from the first install

## Cross-links into the codebase

- `server/src/main.rs` — boot sequence
- `server/src/state.rs` — `AppState` construction (DB, storage, Redis)
- `server/migrations/` — sqlx migration set (run in order, forward only)
- `packaging/systemd/` — systemd unit files (server + capturer)
- `packaging/proxy/` — Caddyfile + Traefik dynamic config
- `deploy/helm/skill-pool/` — Helm chart
- `deploy/terraform/aws/` — AWS Terraform starter
- `.github/workflows/` — CI/CD pipelines
- `docs/ops/` — runbook, capacity, rollback
