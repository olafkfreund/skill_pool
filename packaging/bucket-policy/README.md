# Bucket policy templates

S3 bucket and IAM policies for skill-pool's bundle storage. Two layouts
are supported because they fit different points on the multi-tenancy ↔
data-residency tradeoff.

## When to use which

| Layout                   | When                                                            | Templates                                                                |
|--------------------------|-----------------------------------------------------------------|--------------------------------------------------------------------------|
| **Shared** (one bucket)  | Team / startup tier; all tenants on the same region; cheap.     | `bucket-policy-shared.json` + `AllowAppShared` block in `iam-policy-app.json`     |
| **Dedicated** (one per tenant) | Enterprise tier; data-residency / compliance per tenant; per-region buckets. | `bucket-policy-dedicated.json` + `AllowAppDedicated` block in `iam-policy-app.json` |

Both layouts use the same code path on the app side — `Storage` is
just an `opendal::Operator` configured from `SKILL_POOL_STORAGE_URI`.
The choice is operational, not architectural.

## How the app already scopes by tenant

Every bundle key is prefixed by `tenant_id` (a UUID) in
`server/src/storage.rs::bundle_key`:

```
{tenant_id}/{slug}/{version}.tar.gz
{tenant_id}/drafts/{draft_id}.tar.gz
```

Server-side enforcement: any handler that touches storage goes through
the `TenantCtx` extractor, which fills in `tenant_id` before
`bundle_key` is called. The bucket policy is **defence in depth** —
it stops a leaked credential or a misconfigured CLI from reaching
another tenant's prefix even if the server-side guard is bypassed.

## Shared bucket — applying the policy

```bash
# 1. Edit bucket-policy-shared.json:
#     <BUCKET_NAME>   → skill-pool-prod
#     <APP_ROLE_ARN>  → arn:aws:iam::123456789012:role/skill-pool-server

# 2. Apply
aws s3api put-bucket-policy \
  --bucket skill-pool-prod \
  --policy file://bucket-policy-shared.json

# 3. Verify
aws s3api get-bucket-policy --bucket skill-pool-prod \
  | jq -r .Policy | jq .
```

Pair with `AllowAppShared` + `AllowAppSharedListBucket` in
`iam-policy-app.json` (delete the dedicated Statements before applying).

Server env:

```
SKILL_POOL_STORAGE_URI=s3://skill-pool-prod?region=us-east-1
```

## Dedicated buckets — applying per tenant

For an Enterprise tenant with their own region (data-residency):

```bash
# 1. Create the bucket
aws s3api create-bucket --bucket skill-pool-prod-acme \
  --region eu-west-1 \
  --create-bucket-configuration LocationConstraint=eu-west-1

# 2. Block public access (defaults are good but be explicit)
aws s3api put-public-access-block --bucket skill-pool-prod-acme \
  --public-access-block-configuration \
  BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true

# 3. Versioning (so a DR restore of the DB can re-validate bundle SHA-256s)
aws s3api put-bucket-versioning --bucket skill-pool-prod-acme \
  --versioning-configuration Status=Enabled

# 4. Edit bucket-policy-dedicated.json:
#     <BUCKET_NAME>    → skill-pool-prod-acme
#     <APP_ROLE_ARN>   → arn:aws:iam::123456789012:role/skill-pool-server

# 5. Apply
aws s3api put-bucket-policy \
  --bucket skill-pool-prod-acme \
  --policy file://bucket-policy-dedicated.json

# 6. Add a Statement to the app's IAM policy
#    (duplicate AllowAppDedicated + AllowAppDedicatedListBucket per bucket)
```

Run the app for this tenant in dedicated mode
(`SKILL_POOL_TENANCY_MODE__MODE=dedicated`,
`SKILL_POOL_TENANCY_MODE__TENANT_SLUG=acme` — see
`docs/enterprise/dedicated-mode.md`) pointed at the per-tenant
bucket:

```
SKILL_POOL_STORAGE_URI=s3://skill-pool-prod-acme?region=eu-west-1
```

## Removing the `_comment` key before applying

The templates ship with a `_comment` array explaining each field.
AWS ignores unknown top-level keys, but if your CI policy validator
complains, strip it:

```bash
jq 'del(._comment)' bucket-policy-shared.json | tee bucket-policy-shared.applied.json
```

## Bucket settings these policies assume

The templates focus on *access*; they do not configure the rest. For
the bundle bucket, also enable:

- **TLS-only** — already in the `DenyInsecureTransport` Statement.
- **Versioning** — protects against accidental deletes and lets a DR
  restore (`docs/ops/rollback.md` §4.4) verify bundle SHA-256s
  against the catalog's recorded checksum.
- **SSE** — `aws s3api put-bucket-encryption` with either SSE-S3 or
  SSE-KMS. Bundles contain no secrets (the publish path runs a
  secret scan — see `server/src/bundle.rs`), but at-rest encryption
  is table-stakes for compliance.
- **Lifecycle for `*/drafts/*`** — drafts that don't get promoted
  within N days should expire. A 14-day TTL is a reasonable default;
  configure via `aws s3api put-bucket-lifecycle-configuration` with
  a `Prefix` of `<tenant_id>/drafts/`.
- **Bucket logging** — write S3 access logs to a separate audit
  bucket if your compliance regime requires bundle-access records.

## GCS / Azure equivalents

The same patterns translate to other clouds; replace the IAM verbs
with the cloud-specific names. `opendal` already supports each
backend; the operator's task is to map these statements onto the
cloud's policy DSL.

| AWS S3                  | GCS                                       | Azure Blob                                  |
|-------------------------|-------------------------------------------|---------------------------------------------|
| `s3:GetObject`          | `storage.objects.get`                     | `Microsoft.Storage/.../blobs/read`          |
| `s3:PutObject`          | `storage.objects.create` + `…update`      | `Microsoft.Storage/.../blobs/write`         |
| `s3:DeleteObject`       | `storage.objects.delete`                  | `Microsoft.Storage/.../blobs/delete`        |
| `s3:ListBucket`         | `storage.objects.list`                    | `Microsoft.Storage/.../containers/read`     |
| Bucket Policy           | IAM binding on the bucket resource        | RBAC role assignment on the container       |

## Validating the policy with the AWS policy simulator

Before pushing to prod, simulate the role against the bucket:

```bash
aws iam simulate-principal-policy \
  --policy-source-arn <APP_ROLE_ARN> \
  --action-names s3:GetObject \
  --resource-arns "arn:aws:s3:::skill-pool-prod-acme/<tenant_uuid>/test/1.0.0.tar.gz"
```

Repeat with `--action-names s3:GetObject` and a *different* tenant's
bucket — it must return `EvaluationResult: explicitDeny`.

## Related docs

- `docs/deploy/kubernetes.md` — uses S3 storage in the multi-tenant example.
- `docs/ops/capacity.md` Tier 3 — per-region buckets for data residency.
- `docs/ops/rollback.md` §4.4 — what happens when the bundle store is lost.
- `server/src/storage.rs` — the `Storage::bundle_key` / `draft_bundle_key`
  layout these policies assume.
