# skill-pool Helm chart

Reusable Helm chart for deploying the [skill-pool](https://github.com/olafkfreund/skill_pool)
registry — a multi-tenant Claude Code skill / agent / command catalog —
on any Kubernetes 1.26+ cluster. Bundles:

- **server** Deployment (Rust HTTP API)
- **web** Deployment (SvelteKit portal)
- pre-install / pre-upgrade **migration Job** (`sqlx migrate run`)
- Services, Ingress (nginx + ALB ready), HPA, PDBs
- opt-in **ServiceMonitor** (Prometheus Operator) and **NetworkPolicy**
- IRSA-aware ServiceAccount for AWS EKS

The chart is BYO Postgres and BYO object storage (S3 / GCS / Azure /
local fs). It deliberately ships no umbrella sub-charts so it stays
useful in every environment.

## Quick start

```bash
# 1. create the namespace + secrets out-of-band (or via external-secrets)
kubectl create namespace skill-pool
kubectl -n skill-pool create secret generic skill-pool-db \
    --from-literal=SKILL_POOL_DATABASE_URL='postgres://skillpool:CHANGE_ME@db.internal:5432/skillpool'

# 2. install
helm install skill-pool ./deploy/helm/skill-pool \
    --namespace skill-pool \
    --set server.env.SKILL_POOL_STORAGE_URI='s3://my-bucket?region=eu-west-1' \
    --set web.env.ORIGIN='https://skill-pool.example.com' \
    --set ingress.hosts[0].host='skill-pool.example.com'
```

The pre-install hook runs `sqlx migrate run` before any Deployment
starts. If it fails, the release is aborted and pods are never created.

## Required values

| Key                                | Why                                                                |
|------------------------------------|--------------------------------------------------------------------|
| `server.env.SKILL_POOL_STORAGE_URI`| Where bundle blobs live. Examples: `s3://bucket?region=us-east-1`, `fs:///var/lib/skill-pool`. |
| `web.env.ORIGIN`                   | SvelteKit's adapter-node CSRF check; must match the user-facing URL. |
| `ingress.hosts[0].host`            | The DNS name the Ingress controller routes from.                   |
| A Secret named `skill-pool-db`     | Must export `SKILL_POOL_DATABASE_URL` (and optionally `SKILL_POOL_DATABASE_READ_URL`). |

A second Secret `skill-pool-secrets` is optional; it's where you put
`SKILL_POOL_EMAIL_SECRET_KEY`, SCIM tokens, OIDC client secrets, etc.

## Values reference

| Key                                          | Default                                        | Description |
|----------------------------------------------|------------------------------------------------|-------------|
| `image.server.repository`                    | `ghcr.io/olafkfreund/skill-pool-server`        | Server image. |
| `image.server.tag`                           | `""` (falls back to `.Chart.AppVersion`)       | Override per release. |
| `image.web.repository`                       | `ghcr.io/olafkfreund/skill-pool-web`           | Web image. |
| `image.web.tag`                              | `""` (falls back to `.Chart.AppVersion`)       | Override per release. |
| `server.replicas`                            | `2`                                            | Ignored when HPA enabled. |
| `server.env.*`                               | see `values.yaml`                              | Non-secret env. Rendered into a ConfigMap. |
| `server.envFrom`                             | refs `skill-pool-db`, `skill-pool-secrets`     | Secrets mounted as env. |
| `server.hpa.enabled`                         | `true`                                         | CPU-based HPA, min=2 max=20 target=70%. |
| `server.pdb.enabled`                         | `true`                                         | minAvailable=1. |
| `web.replicas`                               | `2`                                            | Static — no HPA on the web tier. |
| `web.env.ORIGIN`                             | `""`                                           | **REQUIRED.** Full URL incl. scheme. |
| `web.env.SKILL_POOL_API_BASE`                | `""` (computed)                                | When empty, falls back to the in-cluster server Service URL. |
| `migrate.enabled`                            | `true`                                         | Pre-install / pre-upgrade Job. |
| `migrate.image`                              | `ghcr.io/jbergstroem/sqlx-cli:0.8.2`           | Override to a digest in prod. |
| `migrate.copyMigrationsFromServerImage`      | `true`                                         | Uses an initContainer to fetch `/app/migrations` from the server image. |
| `ingress.enabled`                            | `true`                                         |  |
| `ingress.className`                          | `nginx`                                        | Set to `alb` on AWS. |
| `ingress.tls.enabled`                        | `true`                                         | Disable when the Ingress controller terminates TLS for you (ALB+ACM). |
| `aws.irsa.roleArn`                           | `""`                                           | When non-empty, annotates the SA + flips automountToken on. |
| `metrics.serviceMonitor.enabled`             | `false`                                        | Requires Prometheus Operator CRDs. |
| `networkPolicy.enabled`                      | `false`                                        | Default-deny ingress to server/web pods. |
| `serviceAccount.create`                      | `true`                                         |  |
| `podSecurityContext` / `securityContext`     | non-root, RO rootfs, drop ALL caps             | Mirrors the systemd unit hardening. |

For the full set of values + inline docs see [`values.yaml`](./values.yaml).

## AWS-specific install

A canned overlay lives at [`values-aws.yaml`](./values-aws.yaml) (filled
in by Terraform — see `ops/terraform/aws/` once that lands). Typical
flow:

```bash
helm install skill-pool ./deploy/helm/skill-pool \
    --namespace skill-pool \
    -f deploy/helm/skill-pool/values.yaml \
    -f deploy/helm/skill-pool/values-aws.yaml \
    -f my-prod-values.yaml
```

The AWS overlay:

- switches `ingress.className` to `alb`, adds ALB-controller annotations
  (`scheme`, `target-type=ip`, `healthcheck-path`, ACM cert ARN)
- disables chart-managed TLS (ALB terminates via ACM)
- enables ServiceMonitor (AMP-compatible)
- sets `aws.irsa.roleArn` so the SA is annotated with
  `eks.amazonaws.com/role-arn` and pods can assume the role for S3

See [`docs/deploy/aws.md`](../../../docs/deploy/aws.md) for the
end-to-end AWS recipe (Terraform → ECR push → Helm install).

## Migrations

The server binary does **not** auto-migrate on startup
([`docs/ops/rollback.md`](../../../docs/ops/rollback.md) explains why).
The chart's pre-install + pre-upgrade Job runs
`sqlx migrate run --source /migrations` against the bundled migrations.

Mechanics:

1. An init container based on the **server image** copies
   `/app/migrations/*` into an `emptyDir`.
2. The main container (default `ghcr.io/jbergstroem/sqlx-cli:0.8.2`)
   mounts that `emptyDir` and runs `sqlx migrate run`.

This keeps migrations versioned with the server image while not
requiring `sqlx-cli` to be baked into the runtime image.

To bypass the chart-managed migration (e.g. CI handles it):

```bash
helm install … --set migrate.enabled=false
```

## Observability

Pair this chart with:

- [`ops/grafana/skill-pool.json`](../../../ops/grafana/skill-pool.json) — dashboard
- [`ops/alerts/skill-pool.rules.yaml`](../../../ops/alerts/skill-pool.rules.yaml) — Prometheus rules

The `/metrics` route on the server uses the Prometheus text exposition
format. Enable `metrics.serviceMonitor.enabled=true` if you run the
Prometheus Operator.

## Uninstall

```bash
helm uninstall skill-pool -n skill-pool
```

The migration Job hooks self-clean (`hook-delete-policy: hook-succeeded`).
ConfigMaps + Services + Deployments are removed; **Secrets you created
out-of-band are retained** — drop them with `kubectl delete secret` if
that's what you want.

## Testing the chart locally

```bash
helm lint deploy/helm/skill-pool/
helm template skill-pool deploy/helm/skill-pool/ \
    --set image.server.tag=test \
    --set image.web.tag=test \
    --set server.env.SKILL_POOL_STORAGE_URI='s3://x' \
    --set web.env.ORIGIN='https://x.example.com' | kubectl apply --dry-run=client -f -
```
