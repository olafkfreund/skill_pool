# AWS deploy

End-to-end deploy of skill-pool to AWS: EKS for compute, RDS Postgres
16 (with pgvector) for the catalog, S3 for bundle storage, ECR for
container images, an ALB for ingress, and **cert-manager + Let's Encrypt
+ nip.io** for free TLS without buying a domain.

> **TLS strategy.** The default path uses `<dashed-ip>.nip.io` — a free
> wildcard DNS service that resolves `*.<dashed-ip>.nip.io` to any IPv4
> you stick in the host. cert-manager runs in-cluster, watches the
> Ingress, kicks off an HTTP-01 challenge through the ALB, and writes
> the Let's Encrypt cert into a Secret the ALB then serves. **Zero
> domain purchase, zero DNS-provider setup.** When you eventually want
> a real domain, swap `ingress.hosts[].host` to your own and either keep
> cert-manager OR flip to ACM via `var.use_acm_cert = true`.

The Terraform starter is at [`deploy/terraform/aws/`](../../deploy/terraform/aws/);
the Helm chart at [`deploy/helm/skill-pool/`](../../deploy/helm/skill-pool/)
with `values-aws.yaml` as the AWS-specific overlay. GitHub Actions
workflows live in [`.github/workflows/`](../../.github/workflows/).

```
┌──────────────┐  HTTPS  ┌──────────────┐
│  internet    │ ──────► │     ALB      │  ← cert from Let's Encrypt
└──────────────┘         │  (kube-alb)  │     (cert-manager + nip.io)
   *.nip.io DNS          └──────┬───────┘
                                │
                ┌───────────────┴───────────────┐
                ▼                               ▼
        ┌──────────────┐                ┌──────────────┐
        │ skill-pool   │  IRSA → S3     │ skill-pool   │
        │   server     │ ─────────────► │     web      │
        │   (EKS pod)  │                │ (EKS pod)    │
        └──────┬───────┘                └──────────────┘
               │ sqlx
               ▼
        ┌──────────────┐
        │  RDS Postgres │
        │     16        │
        │  (pgvector)   │
        └──────────────┘
```

## 1. Pre-flight

You need:

| Tool          | Version | Why                                              |
|---------------|---------|--------------------------------------------------|
| Terraform     | 1.6+    | Module syntax + provider version constraints.    |
| AWS CLI       | v2      | `eks update-kubeconfig`, secret rotation, ECR.   |
| `kubectl`     | 1.30+   | Cluster ops.                                     |
| `helm`        | 3.14+   | Chart install.                                   |
| `jq`          | any     | Read Terraform outputs cleanly.                  |
| AWS account   | admin   | First apply touches IAM, VPC, EKS.               |
| Route53 zone  | existing | Public hosted zone for your apex (e.g. `skill-pool.example.com`). |

> Keep the zone **outside** this Terraform module. A stray `destroy`
> shouldn't be able to delete your apex. The module references the
> zone by name; it never creates or destroys it.

## 2. Terraform apply

```bash
cd deploy/terraform/aws/

# Edit variables.tf — at minimum:
#   region, azs, route53_zone_name, service_hostnames, github_repository
${EDITOR:-vim} variables.tf

terraform init
terraform plan  -out plan.tfplan
terraform apply plan.tfplan
```

First apply takes **~20 min** (EKS control-plane bootstrap dominates).

### 2a. Targeted apply if anything stalls

The module is wired with `depends_on` so a single apply works, but
network hiccups during the EKS bootstrap can leave the helm provider
unable to talk to the cluster on first pass. If that happens:

```bash
terraform apply -target=module.vpc
terraform apply -target=module.eks
terraform apply -target=module.rds
terraform apply -target=aws_s3_bucket.bundles
terraform apply -target=aws_ecr_repository.server -target=aws_ecr_repository.web
terraform apply -target=aws_iam_role.skill_pool_app -target=aws_iam_role.github_actions
terraform apply -target=aws_acm_certificate_validation.service
terraform apply -target=helm_release.alb_controller
terraform apply    # final converge
```

### 2b. Outputs you'll need

```bash
terraform output -json | tee outputs.json
```

Key fields (see [`deploy/terraform/aws/outputs.tf`](../../deploy/terraform/aws/outputs.tf)
for the full list):

| Output                    | Used in                                  |
|---------------------------|------------------------------------------|
| `cluster_name`            | `aws eks update-kubeconfig`              |
| `irsa_role_arn`           | `values-aws.yaml` serviceAccount         |
| `bundle_storage_uri`      | `values-aws.yaml` server.env             |
| `ecr_server_repo_url`     | `values-aws.yaml` image.server + GH CI   |
| `ecr_web_repo_url`        | `values-aws.yaml` image.web + GH CI      |
| `acm_certificate_arn`     | `values-aws.yaml` ingress annotation     |
| `github_actions_role_arn` | GitHub repo secret `AWS_ROLE_ARN`        |
| `rds_password_secret_arn` | k8s Secret bootstrap (§5)                |

## 3. First-time bootstrap (out-of-Terraform)

Three things are deliberately not in Terraform:

1. **The RDS password as a k8s Secret.** The plaintext lives in Secrets
   Manager (`rds_password_secret_arn`). The app reads the password
   via the `SKILL_POOL_DATABASE_URL` env var, which is sourced from a
   k8s Secret. Bridging the two is your call — three options:
   - **kubectl one-shot** (simple, fine for v1): pull the secret with
     the AWS CLI and `kubectl create secret`.
   - **External Secrets Operator**: deploy ESO into the cluster, point
     a `SecretStore` at AWS Secrets Manager, declare an `ExternalSecret`
     that materialises `skill-pool-env`. The k8s Secret is auto-kept in
     sync with the source.
   - **Helm post-install hook with the AWS CLI**: not recommended —
     bundles secrets into the chart release lifecycle.

2. **The email AES key (`SKILL_POOL_EMAIL_SECRET_KEY`).** Used to
   encrypt per-tenant SMTP / branding credentials at rest (see
   `server/src/email_branding.rs`). Generate once per environment and
   stash in Secrets Manager:
   ```bash
   openssl rand -hex 32 | aws secretsmanager create-secret \
     --name skill-pool-prod/email-secret-key \
     --secret-string file:///dev/stdin
   ```
   Then bridge into the same `skill-pool-env` k8s Secret as the DB DSN.

3. **Per-tenant SMTP credentials** for tenants that bring their own.
   These are managed via the `admin tenant-email-config` CLI hitting
   the running app; nothing to set up at deploy time. See
   [`docs/enterprise/branded-emails.md`](../enterprise/branded-emails.md).

## 4. Connect to the cluster

```bash
aws eks update-kubeconfig \
  --region "$(terraform -chdir=deploy/terraform/aws output -raw region)" \
  --name   "$(terraform -chdir=deploy/terraform/aws output -raw cluster_name)"

kubectl get nodes
# NAME                                       STATUS   ROLES    AGE   VERSION
# ip-10-42-10-23.eu-west-1.compute.internal  Ready    <none>   3m    v1.30.0-eks-...
# ip-10-42-11-87.eu-west-1.compute.internal  Ready    <none>   3m    v1.30.0-eks-...
```

The AWS Load Balancer Controller was installed by Terraform
(`helm_release.alb_controller`). Confirm:

```bash
kubectl -n kube-system get deploy aws-load-balancer-controller
# NAME                           READY   UP-TO-DATE   AVAILABLE   AGE
# aws-load-balancer-controller   2/2     2            2           5m
```

## 5. Bootstrap k8s — namespace + Secrets

```bash
kubectl create namespace skill-pool

# --- DB DSN + email key Secret ---
DB_DSN="$(aws secretsmanager get-secret-value \
  --secret-id "$(terraform -chdir=deploy/terraform/aws output -raw rds_password_secret_arn)" \
  --query SecretString --output text | jq -r .dsn)"

EMAIL_KEY="$(aws secretsmanager get-secret-value \
  --secret-id skill-pool-prod/email-secret-key \
  --query SecretString --output text)"

kubectl -n skill-pool create secret generic skill-pool-env \
  --from-literal=SKILL_POOL_DATABASE_URL="$DB_DSN" \
  --from-literal=SKILL_POOL_EMAIL_SECRET_KEY="$EMAIL_KEY"
```

If you're using **External Secrets Operator** instead, see
[the ESO quickstart](https://external-secrets.io/) — the `SecretStore`
points at the same two ARNs and ESO keeps the k8s Secret in sync.

## 6. Run sqlx migrations

The server **does not auto-migrate on startup** (same rule as the
single-node and generic k8s deploys). Run migrations as a one-shot job
before the first chart install:

```bash
kubectl -n skill-pool run sqlx-migrate \
  --rm -it --restart=Never \
  --image "$(terraform -chdir=deploy/terraform/aws output -raw ecr_server_repo_url):$IMAGE_TAG" \
  --env "SKILL_POOL_DATABASE_URL=$DB_DSN" \
  --command -- /usr/local/bin/skill-pool-server migrate
```

> The pgvector extension is `CREATE EXTENSION IF NOT EXISTS vector` in
> the migration set — RDS Postgres 16 ships pgvector preloaded, so it
> just works.

## 7. First deploy

### Option A — via GitHub Actions (recommended)

Tag a release: `git tag v0.1.0 && git push origin v0.1.0`. The
`build.yml` workflow (sister subagent's domain) builds both images,
pushes to ECR, and the `deploy.yml` workflow does the `helm upgrade`.
See `.github/workflows/` and its README.

### Option B — manual, first time

```bash
# Fill in the CUSTOMISE markers in values-aws.yaml with terraform outputs.
${EDITOR:-vim} deploy/helm/skill-pool/values-aws.yaml

# Build + push images. (Local docker — for arm64 hosts add --platform.)
ACCOUNT="$(aws sts get-caller-identity --query Account --output text)"
REGION="$(terraform -chdir=deploy/terraform/aws output -raw region)"
aws ecr get-login-password --region "$REGION" \
  | docker login --username AWS --password-stdin "$ACCOUNT.dkr.ecr.$REGION.amazonaws.com"

docker build -t "$ACCOUNT.dkr.ecr.$REGION.amazonaws.com/skill-pool-server:v0.1.0" \
  -f server/Dockerfile .
docker push "$ACCOUNT.dkr.ecr.$REGION.amazonaws.com/skill-pool-server:v0.1.0"

docker build -t "$ACCOUNT.dkr.ecr.$REGION.amazonaws.com/skill-pool-web:v0.1.0" \
  -f web/Dockerfile web
docker push "$ACCOUNT.dkr.ecr.$REGION.amazonaws.com/skill-pool-web:v0.1.0"

# Install the chart.
helm install skill-pool ./deploy/helm/skill-pool \
  -f deploy/helm/skill-pool/values-aws.yaml \
  -n skill-pool --create-namespace
```

### Verify rollout

```bash
kubectl -n skill-pool rollout status deploy/skill-pool-server
kubectl -n skill-pool rollout status deploy/skill-pool-web
kubectl -n skill-pool get pods
```

## 8. DNS via nip.io + Let's Encrypt cert (default, free)

The default deploy uses **nip.io** as a wildcard DNS service so you don't
need to buy a domain or configure Route53. `<anything>.<dashed-ipv4>.nip.io`
resolves to the IPv4 in the middle.

### Step 1 — find the ALB IP

The chart's Ingress causes the ALB Controller to provision an ALB.
Resolve its DNS name to a public IP:

```bash
ALB_HOST="$(kubectl -n skill-pool get ingress skill-pool \
  -o jsonpath='{.status.loadBalancer.ingress[0].hostname}')"
# k8s-skillpoo-skillpoo-abc123def.eu-west-1.elb.amazonaws.com

ALB_IP="$(dig +short "$ALB_HOST" | head -n1)"
echo "$ALB_IP"
# 54.220.123.45
```

(ALBs are technically dual-stack with multiple IPs; any one works for
nip.io. The IPs rotate occasionally — re-resolve if you see TLS
issuance failures days later.)

### Step 2 — derive the nip.io host

```bash
DASHED_IP="${ALB_IP//./-}"
PORTAL_HOST="skill-pool.${DASHED_IP}.nip.io"
echo "$PORTAL_HOST"
# skill-pool.54-220-123-45.nip.io
```

### Step 3 — re-deploy with the real host

The first deploy used a placeholder host so the chart could render. Now
that the ALB is up, set the real host and re-upgrade:

```bash
helm upgrade skill-pool ./deploy/helm/skill-pool \
  -f deploy/helm/skill-pool/values-aws.yaml \
  --namespace skill-pool \
  --set ingress.hosts[0].host="$PORTAL_HOST" \
  --reuse-values
```

### Step 4 — wait for Let's Encrypt to issue the cert

cert-manager (installed by `deploy/terraform/aws/cert-manager.tf`)
watches the Ingress, creates a `Certificate` resource, runs the HTTP-01
challenge through the ALB on port 80, and writes the issued cert into
the `skill-pool-tls` Secret. Watch it:

```bash
kubectl -n skill-pool get certificate -w
# Wait until READY=True (typically 30-120 seconds).

# If it stalls, inspect:
kubectl -n skill-pool describe certificate skill-pool-tls
kubectl -n skill-pool describe certificaterequest
kubectl -n skill-pool describe order
kubectl -n skill-pool describe challenge
```

**While iterating**, point the chart at the LE staging issuer (much
higher rate limits, untrusted cert):

```bash
helm upgrade skill-pool ./deploy/helm/skill-pool \
  -f deploy/helm/skill-pool/values-aws.yaml \
  --namespace skill-pool \
  --set ingress.annotations.'cert-manager\.io/cluster-issuer'=letsencrypt-staging \
  --reuse-values
```

Then flip back to `letsencrypt-prod` once the path works end-to-end.

### Step 5 — per-tenant subdomains

Tenants get their own subdomain on the same nip.io hostname:

```text
acme.54-220-123-45.nip.io   → resolves to ALB → tenant acme
globex.54-220-123-45.nip.io → resolves to ALB → tenant globex
```

For each tenant subdomain you also want LE-issued, list it in
`ingress.hosts[]` so cert-manager issues a SAN cert. cert-manager
will batch the host list into one `Certificate` resource and one
challenge per host (Let's Encrypt allows up to 100 SAN names per cert).

### What changes when you switch to a real domain

When you outgrow nip.io (production, branding, SAN-cert rate limits):

1. Buy a domain + add a Route53 hosted zone.
2. Set `var.route53_zone_name` in Terraform and `var.use_nip_io = false`.
3. Re-apply Terraform — it'll create the Route53 ALIAS records pointing
   at the ALB.
4. Change `ingress.hosts[].host` to your real domain.
5. Optionally `terraform apply -var use_acm_cert=true` to swap to an
   ACM-issued cert and drop cert-manager (slightly cheaper / fully
   managed by AWS; cert-manager + LE works fine too).

> The `HostedZoneId` `Z32O12XQLNTSW2` is the well-known eu-west-1
> ALB zone — substitute your region's value from
> https://docs.aws.amazon.com/general/latest/gr/elb.html.

A `ExternalDNS` deployment will automate this for you if you don't
want to do it by hand; out of scope for v1.

## 9. First tenant

```bash
kubectl -n skill-pool exec deploy/skill-pool-server -- \
  skill-pool-server admin tenant-create \
    --slug acme --name "Acme Inc." --plan team

kubectl -n skill-pool exec deploy/skill-pool-server -- \
  skill-pool-server admin token-create \
    --tenant acme --name bootstrap
# → raw token printed once. Use as Authorization: Bearer …
```

## 10. Verify end-to-end

```bash
# 1. Apex
curl -fsS https://skill-pool.example.com/v1/healthz
# {"status":"ok","db":"ok","storage":"ok"}

# 2. Tenant subdomain (web UI)
curl -fsS -o /dev/null -w "%{http_code}\n" https://acme.skill-pool.example.com/login
# 200

# 3. Publish a smoke skill
skill-pool publish ./examples/hello-skill --version 0.1.0 \
  --token "$BOOTSTRAP_TOKEN" \
  --base https://acme.skill-pool.example.com

# 4. Confirm the bundle landed in S3
aws s3 ls "s3://$(terraform -chdir=deploy/terraform/aws output -raw bundle_bucket_name)/" --recursive | head
```

## 11. Day-2 ops

Forward pointers:

- [**Runbook**](../ops/runbook.md) — on-call procedures, SLOs, alert
  acknowledgement playbooks.
- [**Capacity planning**](../ops/capacity.md) — tier-by-tier sizing
  guidance, the cost curve, when to flip RDS Multi-AZ on.
- [**Rollback**](../ops/rollback.md) — forward-only migrations, DR from
  RDS snapshots + S3 versioning.
- [**Generic k8s deploy**](./kubernetes.md) — the cloud-agnostic
  reference these AWS specifics override.
- **GitHub Actions** — see `.github/workflows/` for the build + deploy
  pipeline (the sister subagent's deliverable).
- **Per-tenant data residency** — [`docs/enterprise/data-residency.md`](../enterprise/data-residency.md)
  uses the same bucket-policy templates this Terraform extends.

### Future: RDS IAM auth

This deploy uses password auth via the k8s Secret. RDS IAM auth (IAM
principal → 15-minute token → Postgres) needs:

1. `iam_database_authentication_enabled = true` on the RDS instance.
2. The IRSA role (`aws_iam_role.skill_pool_app`) granted
   `rds-db:connect` on `arn:aws:rds-db:<region>:<account>:dbuser:<db-resource-id>/skillpool`.
3. App-side: `sqlx` reconnect on token expiry (not yet implemented —
   the connection pool would need a `before_acquire` hook).

Not blocking for v1; revisit when (2) and (3) are easy.

### Future: WAF + Shield + GuardDuty

Account-level controls; should pre-exist or get layered on per
compliance regime. The ALB Controller supports
`alb.ingress.kubernetes.io/wafv2-acl-arn` — just paste the ACL ARN into
`values-aws.yaml`'s ingress annotations.

## 12. Cost

Lean baseline (~$248/mo, see [`deploy/terraform/aws/README.md`](../../deploy/terraform/aws/README.md#cost-lean-baseline-eu-west-1-may-2026-list-prices)
for the breakdown):

| Component        | Monthly  |
|------------------|----------|
| EKS control plane | $73     |
| 2× t3.medium     | $60      |
| RDS t4g.medium   | $50      |
| ALB              | $22      |
| NAT (single AZ)  | $32      |
| Misc (S3, ECR, R53, SM) | $11 |
| **Total**        | **~$248**|

For HA: Multi-AZ RDS + per-AZ NAT + a third worker node ≈ **+$110/mo**
(~$360/mo total).

For dev/staging: drop to a single Spot worker, single-AZ everything,
no NAT (private subnets behind VPC endpoints) ≈ **~$130/mo**.

## Troubleshooting

| Symptom                                          | Likely cause / fix                                                                 |
|--------------------------------------------------|------------------------------------------------------------------------------------|
| `helm install` hangs on ALB controller readiness | The controller CRDs land before the controller pod is ready. Retry once.            |
| Pod can't reach S3: `403 Forbidden`              | IRSA annotation typo. `kubectl describe sa skill-pool` and confirm the role ARN.   |
| `psql: FATAL: password authentication failed`    | k8s Secret is stale after a Terraform password rotation. Recreate `skill-pool-env`. |
| Ingress stays `<pending>`                        | Subnet tags missing. `vpc.tf` sets them; check `kubernetes.io/role/elb=1` on public subnets. |
| `CREATE EXTENSION vector` errors during migrate  | RDS minor version too old. Bump `rds_postgres_version` ≥ 16.3.                     |
| `terraform apply` errors with `Unauthorized` on EKS | Your IAM principal isn't in `module.eks.access_entries`. The bootstrap principal gets cluster-admin automatically; everyone else needs an explicit entry. |

If something else breaks, `kubectl -n skill-pool describe pod` +
`kubectl -n skill-pool logs` are the first two commands. Almost all
real issues surface as IRSA mis-binding (S3 errors) or stale Secrets
(DB errors).
