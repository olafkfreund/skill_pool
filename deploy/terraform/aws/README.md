# skill-pool · AWS Terraform starter

A **starting kit**, not a production module. Fork it, edit the
`# CUSTOMISE:` markers, and apply. The goal is to get a skill-pool
control plane up on AWS in an afternoon, then iterate.

The full end-to-end deploy walk-through lives in
[`docs/deploy/aws.md`](../../../docs/deploy/aws.md) — read that first.
This README documents the Terraform layout itself.

## What you get

| Resource         | File                  | Module                           |
|------------------|-----------------------|----------------------------------|
| Two-AZ VPC       | `vpc.tf`              | `terraform-aws-modules/vpc/aws ~> 5.13` |
| EKS cluster      | `eks.tf`              | `terraform-aws-modules/eks/aws ~> 20.24` |
| RDS Postgres 16  | `rds.tf`              | `terraform-aws-modules/rds/aws ~> 6.7` |
| S3 bundle bucket | `s3.tf`               | hand-rolled (small surface)      |
| ECR repos × 2    | `ecr.tf`              | hand-rolled                      |
| IRSA role (app)  | `iam-irsa.tf`         | hand-rolled                      |
| GitHub OIDC role | `iam-github-oidc.tf`  | hand-rolled                      |
| ACM cert         | `acm.tf`              | hand-rolled                      |
| Route53 DNS-validation records | `route53.tf` | hand-rolled |
| ALB Controller   | `alb-controller.tf`   | hand-rolled IAM + `helm_release` |

### Why community modules where I used them

`terraform-aws-modules/{vpc,eks,rds}` are the de-facto standards. They
handle the dozen tags / addons / security groups / monitoring knobs each
service wants, and they get patched faster than I can keep up with.
Hand-rolling them means re-discovering bugs the community already fixed.

### Why hand-rolled where I didn't

S3 / ECR / IAM / ACM are 30-line resources. A module wrapper adds more
surface area than it saves. IRSA and the GitHub OIDC trust shape are
opinionated to skill-pool specifically — they belong in this repo, not
in a generic upstream.

## Prerequisites

- **Terraform 1.6+**
- **AWS CLI v2**, authenticated to an account where you have admin (the
  bootstrap apply touches IAM + VPC + EKS).
- **kubectl + helm** for the post-apply k8s bootstrap.
- A **public Route53 hosted zone** (the apex domain for your service).
  This module references it, it does not create it — managing the zone
  itself out-of-band is safer (no accidental destroy).
- An **S3 bucket + DynamoDB lock table** for Terraform state. Stub
  block is in `versions.tf`; commented out so `init` works locally.

## Quick start

```bash
cd deploy/terraform/aws/

# 1. Edit variables.tf — at minimum:
#      region              → your region
#      azs                 → must match region
#      route53_zone_name   → your apex
#      service_hostnames   → your hostnames
#      github_repository   → owner/repo

terraform init
terraform plan  -out plan.tfplan
terraform apply plan.tfplan
```

First apply takes ~20 minutes (EKS control plane is slow). RDS and
S3 are quick; ECR is instant.

## Apply order (when you can't `apply` everything at once)

The module is written so a single `terraform apply` works — providers
and `depends_on` chains line up. But if you're piecemeal:

```bash
terraform apply -target=module.vpc
terraform apply -target=module.eks
terraform apply -target=aws_ecr_repository.server -target=aws_ecr_repository.web
terraform apply -target=module.rds
terraform apply -target=aws_s3_bucket.bundles
terraform apply -target=aws_iam_role.skill_pool_app -target=aws_iam_role.github_actions
terraform apply -target=aws_acm_certificate_validation.service
terraform apply -target=helm_release.alb_controller
terraform apply        # converge any stragglers
```

## What you'll need to paste elsewhere after apply

```bash
terraform output -json | jq
```

Use the outputs to fill in:

- **GitHub repo secrets** (`Settings → Secrets and variables → Actions`):
  - `AWS_ROLE_ARN` ← `github_actions_role_arn`
  - `AWS_REGION` ← `region`
  - `ECR_SERVER_REPO` ← `ecr_server_repo_url`
  - `ECR_WEB_REPO` ← `ecr_web_repo_url`
  - `EKS_CLUSTER_NAME` ← `cluster_name`
- **Helm values** (`deploy/helm/skill-pool/values-aws.yaml`):
  - `serviceAccount.annotations.eks.amazonaws.com/role-arn` ← `irsa_role_arn`
  - `ingress.annotations.alb.ingress.kubernetes.io/certificate-arn` ← `acm_certificate_arn`
  - `server.env.SKILL_POOL_STORAGE_URI` ← `bundle_storage_uri`
  - `image.server.repository` ← `ecr_server_repo_url`
  - `image.web.repository` ← `ecr_web_repo_url`

## Cost (lean baseline, eu-west-1, May 2026 list prices)

| Item                            | Monthly |
|---------------------------------|---------|
| EKS control plane               | $73     |
| 2× t3.medium worker nodes       | $60     |
| RDS `db.t4g.medium` single-AZ   | $50     |
| 50 GB gp3 RDS storage           | $5      |
| ALB                             | $22     |
| NAT gateway (single-AZ)         | $32     |
| ECR storage + scans (light)     | $2      |
| S3 (50 GB bundles, light reqs)  | $2      |
| Route53 zone + queries          | $1      |
| Secrets Manager (1 secret)      | $0.40   |
| **Total**                       | **~$248/mo** |

For HA double the NAT (~+$32), flip RDS to Multi-AZ (~+$50), and add a
third node (~+$30). Roughly **$360/mo**.

For dev/staging, drop to one node + Spot (~$130/mo).

## What's intentionally NOT here

- **The Route53 hosted zone** — managed out-of-band; this module references it.
- **The RDS password** — generated by `random_password`, stored in Secrets
  Manager, *referenced* by the app via IRSA. The plaintext never leaves
  Terraform state on disk.
- **The Helm chart itself** — that's `deploy/helm/skill-pool/` (sibling).
  Only `values-aws.yaml` lives in this Terraform tree's neighbouring dir.
- **GitHub Actions workflows** — `.github/workflows/` is the sister
  subagent's domain.
- **Cert-manager** — ALB does TLS with ACM directly. No cert-manager.
- **WAF, Shield Advanced, GuardDuty** — flip them on per-account, not
  per-deployment. Out of scope here.
- **Read replicas** — start single-writer; add a replica + set
  `SKILL_POOL_DATABASE_READ_URL` later. The app already supports it.
- **VPC Flow Logs / CloudTrail / AWS Config** — standard "platform"
  account-level controls; should pre-exist.

## Variables you'll most likely touch

| Variable                  | Default                        | Why you'd change it                       |
|---------------------------|--------------------------------|-------------------------------------------|
| `region`                  | `eu-west-1`                    | Match your tenant base / data-residency.  |
| `azs`                     | `[…1a, …1b]`                   | Must match region.                        |
| `route53_zone_name`       | `skill-pool.example.com`       | Your apex.                                |
| `service_hostnames`       | apex + wildcard                | Your apex + wildcard.                     |
| `github_repository`       | `olafkfreund/skill_pool`       | Your fork.                                |
| `rds_multi_az`            | `false`                        | Flip true for prod.                       |
| `node_instance_types`     | `["t3.medium"]`                | Bump to `m6i.large` when traffic shows up.|
| `bundle_bucket_use_kms`   | `false`                        | Compliance regimes that demand CMK.       |

## Sanity checks

```bash
terraform fmt -check -recursive
terraform validate
```

CI runs these on every change to `deploy/terraform/**`. Add a `tflint`
hop if you want stricter linting.

## Destroy

```bash
terraform destroy
```

RDS has `deletion_protection = true` by default — flip it in
`variables.tf` first. S3 buckets with versioning will keep the version
history; empty + force-delete out-of-band if needed.

## Related

- [`docs/deploy/aws.md`](../../../docs/deploy/aws.md) — full deploy walk-through.
- [`docs/deploy/kubernetes.md`](../../../docs/deploy/kubernetes.md) — cloud-agnostic k8s reference.
- [`packaging/bucket-policy/`](../../../packaging/bucket-policy/) — the S3/IAM templates this module adapts.
- [`docs/enterprise/data-residency.md`](../../../docs/enterprise/data-residency.md) — per-tenant region overrides.
