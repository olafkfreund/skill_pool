# GitHub Actions for skill-pool

This document describes the four workflows that ship skill-pool to AWS
EKS, how they authenticate to AWS (OIDC, no static keys), and the
secrets + variables you need to configure in the GitHub UI.

> All AWS-touching workflows run with `permissions: id-token: write`
> and assume an IAM role via short-lived OIDC tokens. There are **no
> long-lived AWS keys stored in GitHub.** The IAM role's trust policy
> is defined in `deploy/terraform/aws/` — see that sibling document
> for the AWS-side setup.

## Workflow inventory

| Workflow             | File                              | Triggers                                | What it does                                              |
| -------------------- | --------------------------------- | --------------------------------------- | --------------------------------------------------------- |
| **CI**               | `.github/workflows/ci.yml`        | push to `main`, all PRs                 | fmt + clippy + workspace tests, web lint/check/test, helm lint + dry-run |
| **Build & push**     | `.github/workflows/build.yml`     | push to `main`, tag `v*`, manual        | Build + push both images (server + web) to ECR            |
| **Deploy to EKS**    | `.github/workflows/deploy.yml`    | tag `v*`, manual                        | `helm upgrade` against EKS + smoke-test, auto-rollback on failure |
| **DB migrations**    | `.github/workflows/migrate.yml`   | manual only (workflow_dispatch)         | Break-glass: run `sqlx migrate run` from a one-shot pod   |

Pull requests **never** see AWS credentials. CI runs unit/integration
tests against an in-runner Postgres service container and an optional
helm-lint job (gated on the chart existing in tree). Cloud access is
limited to `build.yml`, `deploy.yml`, `migrate.yml`.

## Secrets and variables

Configure these in GitHub: **Settings → Secrets and variables →
Actions**. Use *Secrets* for sensitive values, *Variables* for
non-sensitive configuration.

### Repository-level secrets

| Name           | Purpose                                                                 |
| -------------- | ----------------------------------------------------------------------- |
| `AWS_ROLE_ARN` | Full ARN of the IAM role GitHub assumes via OIDC. Format: `arn:aws:iam::<account-id>:role/skill-pool-github-actions`. The trust policy on this role must allow `token.actions.githubusercontent.com` for this repository (see Terraform sibling). |

That is the **only** repository secret required. No `AWS_ACCESS_KEY_ID`,
no `AWS_SECRET_ACCESS_KEY`, no `AWS_SESSION_TOKEN`. If you find
yourself adding one, stop — the design has been bypassed.

### Repository-level variables

| Name                | Example                          | Used by              | Purpose |
| ------------------- | -------------------------------- | -------------------- | ------- |
| `AWS_REGION`        | `eu-west-1`                      | build, deploy, migrate | The region of the EKS cluster + ECR repos. |
| `ECR_REPO_SERVER`   | `skill-pool/server`              | build, migrate       | ECR repository name for the Rust server image (without registry prefix). |
| `ECR_REPO_WEB`      | `skill-pool/web`                 | build                | ECR repository name for the SvelteKit web image. |
| `EKS_CLUSTER_NAME`  | `skill-pool-prod`                | deploy, migrate      | EKS cluster name passed to `aws eks update-kubeconfig`. |
| `HELM_RELEASE_NAME` | `skill-pool`                     | deploy, migrate      | Helm release name; also the prefix for `Deployment` names. |
| `HELM_NAMESPACE`    | `skill-pool`                     | deploy, migrate      | Kubernetes namespace the release lives in. Created on first deploy. |
| `PUBLIC_HOSTNAME`   | `skill-pool.example.com`         | deploy               | Hostname used by the post-deploy smoke test (`https://${HOSTNAME}/v1/healthz`). |

### Environment-level reviewers (recommended)

In **Settings → Environments**, create two GitHub Environments:

- `production` — required by `deploy.yml`. Add required reviewers if
  you want manual approval on every tag push (recommended).
- `prod` — required by `migrate.yml` when `target=prod`. Always add
  required reviewers; the migrate workflow is the most dangerous one
  in this repo.
- `staging` — required by `migrate.yml` when `target=staging`.
  Optionally restrict to specific branches.

You can scope `AWS_ROLE_ARN` per environment (e.g. a separate role for
production that's locked down to one cluster + one ECR account) — the
workflows look up `secrets.AWS_ROLE_ARN` which is automatically
overridden by an environment-scoped secret of the same name.

## OIDC trust shape (GitHub side)

The workflows declare:

```yaml
permissions:
  id-token: write     # required to mint the OIDC JWT
  contents: read      # for actions/checkout
```

and authenticate with:

```yaml
- uses: aws-actions/configure-aws-credentials@v4
  with:
    role-to-assume: ${{ secrets.AWS_ROLE_ARN }}
    aws-region:     ${{ vars.AWS_REGION }}
    role-session-name: gha-<workflow>-${{ github.run_id }}
```

GitHub mints a JWT signed by `token.actions.githubusercontent.com` and
hands it to AWS STS. STS validates the JWT against the IAM role's
trust policy and returns short-lived credentials (default: 1h). All
subsequent AWS calls in the job use those credentials.

The AWS-side configuration (OIDC provider, trust policy, role
permissions) lives in `deploy/terraform/aws/` — see that document for
the trust policy `Condition` block that scopes the role to this
repository.

## Manual operations

### Trigger a deploy without pushing a tag

```
GitHub → Actions → "Deploy to EKS" → Run workflow
  version: v0.2.0    # the image tag to deploy
```

The workflow assumes the images at `<ECR>/skill-pool/server:<version>`
and `<ECR>/skill-pool/web:<version>` already exist. If they don't,
trigger `build.yml` first (or push the corresponding git tag and let
`build.yml` fire automatically).

### Run migrations (break-glass)

```
GitHub → Actions → "DB migrations (break-glass)" → Run workflow
  target:    prod
  image_tag: v0.2.0
```

The workflow:

1. Assumes the AWS role via OIDC.
2. Fetches the DSN from the in-cluster `skill-pool-db` Secret
   (`SKILL_POOL_DATABASE_URL` key).
3. Pulls the `skill-pool-server:<image_tag>` image to confirm it
   exists.
4. Launches a one-shot Pod in the cluster that runs
   `/usr/local/bin/skill-pool-server migrate`.

**When to use this:**

- The Helm chart's `pre-upgrade` migration hook failed and you need to
  retry without redeploying the app.
- You need to apply a single migration manually to align state before
  a deploy (rare).
- You're aligning a staging cluster's schema with prod.

**When NOT to use this:**

- For a normal release. The `pre-upgrade` Helm hook handles this
  automatically on `helm upgrade`.
- To roll a migration *back*. There are no down-migrations
  (see `docs/ops/rollback.md` §1).

### Rollback a deploy

`deploy.yml` rolls itself back automatically if `helm upgrade`
succeeded but the rollout-status / smoke-test step failed. You will
see a `helm rollback on failure` step run in that case.

To roll back manually (e.g. an issue caught after the workflow
returned green):

```
aws eks update-kubeconfig --region "$AWS_REGION" --name "$EKS_CLUSTER_NAME"
helm history skill-pool -n skill-pool
helm rollback skill-pool <REVISION> -n skill-pool --wait --timeout 5m
kubectl rollout status deployment/skill-pool-server -n skill-pool
kubectl rollout status deployment/skill-pool-web    -n skill-pool
```

The `helm rollback` reverts the app, not the schema. Per
`docs/ops/rollback.md` §1, schema rollbacks are forward-only (or a
point-in-time DB restore for §4.3 disasters); the old binary reads
the new schema fine because all schema changes are additive.

## Image tagging convention

| Trigger             | Tags pushed                                              |
| ------------------- | -------------------------------------------------------- |
| Push to `main`      | `<git-sha>`, `latest`                                    |
| Push of tag `v*`    | `<git-sha>`, `latest`, `<git-ref-name>` (e.g. `v0.2.0`)  |
| Manual workflow run | `<git-sha>`, `latest` (+ tag if you ran on a tag ref)    |

`<git-sha>` is `github.sha` — always 40 hex chars, always unique.
That's the tag the deploy workflow uses by default. `latest` is for
human convenience (`docker pull …:latest` in a dev shell) and should
never be referenced by the cluster — `values-aws.yaml` pins specific
tags via the `image.server.tag` / `image.web.tag` keys.

`<git-ref-name>` is the semver-style tag, e.g. `v0.2.0`. That's what
release artifacts ship — when ops files a deploy ticket for "v0.2.0",
that's the tag they mean.

## Local sanity checks (before pushing)

```bash
# Format + lint (matches the rust job)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Workspace tests (matches the rust job, but uses your local Postgres)
cargo test --workspace

# Web (matches the web job)
cd web && npm ci && npm run lint && npm run check && npm test

# Helm (matches the helm job — only meaningful once the chart lands)
helm lint deploy/helm/skill-pool/
helm template skill-pool deploy/helm/skill-pool/ \
  --set image.server.tag=test --set image.web.tag=test \
  --set server.env.SKILL_POOL_STORAGE_URI='s3://x' \
  --set web.env.ORIGIN='https://x.example.com' \
  | kubectl apply --dry-run=client -f -
```

## Maintenance

- Dependabot raises weekly PRs against `cargo` (workspace root),
  `npm` (`web/`), and `github-actions` (workflow files). Patch/minor
  bumps are grouped; majors come as individual PRs.
- When a workflow action goes stale (a new major), Dependabot will
  raise a PR with the upgrade. Review the changelog for breaking
  changes before merging — especially for
  `aws-actions/configure-aws-credentials` (OIDC mechanics) and
  `docker/build-push-action` (cache backends).

## Cross-references

- `deploy/helm/skill-pool/` — the Helm chart (owned by the sibling
  Helm subagent). The `image.server.tag` / `image.web.tag` keys this
  doc references live there.
- `deploy/terraform/aws/` — IAM role + OIDC provider + ECR + EKS
  Terraform (owned by the sibling AWS subagent).
- `docs/ops/rollback.md` — operating model for forward-only
  migrations and the four failure-mode rollback recipes.
- `docs/deploy/kubernetes.md` — manual k8s deploy (the contract the
  Helm chart implements).
