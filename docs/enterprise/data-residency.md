# Data residency

Per-tenant region tag + per-tenant bundle-storage override, so an
Enterprise tenant in a regulated region can pin their bundles to a
region-local bucket while the rest of the deployment stays on the
default backend. Postgres rows still live in the shared DB; only the
bundle bytes move. For full physical isolation (separate DB, separate
host) use [dedicated mode](./dedicated-mode.md) instead.

## What this gives you

- **`tenants.region`** — a free-form tag (`"eu-west-1"`, `"ap-southeast-2"`,
  …). Visible in admin tooling and audit; not enforced by the server.
  Treat it as metadata for human operators.
- **`tenants.storage_uri`** — overrides the global
  `SKILL_POOL_STORAGE_URI` for this tenant only. Every bundle
  read/write for the tenant goes through the override; everyone else
  rides the default.

When `storage_uri IS NULL` (the default — every tenant before you set
this) the server behaves exactly as it did before. Zero behavioural
change for existing deploys.

## Set it up

### 1. Provision the per-tenant bucket

```bash
# Example: EU region for tenant "acme"
aws s3api create-bucket --bucket skill-pool-acme-eu \
  --region eu-west-1 \
  --create-bucket-configuration LocationConstraint=eu-west-1

aws s3api put-public-access-block --bucket skill-pool-acme-eu \
  --public-access-block-configuration \
  BlockPublicAcls=true,IgnorePublicAcls=true,BlockPublicPolicy=true,RestrictPublicBuckets=true

aws s3api put-bucket-versioning --bucket skill-pool-acme-eu \
  --versioning-configuration Status=Enabled
```

Apply the per-tenant bucket policy from
[`packaging/bucket-policy/bucket-policy-dedicated.json`](../../packaging/bucket-policy/bucket-policy-dedicated.json):

```bash
aws s3api put-bucket-policy \
  --bucket skill-pool-acme-eu \
  --policy file://packaging/bucket-policy/bucket-policy-dedicated.json
```

(See [`packaging/bucket-policy/README.md`](../../packaging/bucket-policy/README.md)
for the IAM role-side counterpart.)

### 2. Wire the override

```bash
skill-pool-server admin tenant-residency \
  --slug acme \
  --region eu-west-1 \
  --storage-uri 's3://skill-pool-acme-eu?region=eu-west-1'
```

The CLI validates the URI synchronously — typos abort the update before
they hit the DB. The server may need a restart (or wait until the cache
TTL elapses, which for v1 is "never" — the cache is process-lifetime)
for the new URI to take effect; the CLI reminds you.

Pass either flag in isolation:

```bash
# Only set the region tag (metadata; doesn't move bundles)
skill-pool-server admin tenant-residency --slug acme --region eu-west-1

# Only override the storage URI; leave region tag alone
skill-pool-server admin tenant-residency --slug acme \
  --storage-uri 's3://skill-pool-acme-eu?region=eu-west-1'
```

Pass an empty string to clear a value:

```bash
skill-pool-server admin tenant-residency --slug acme --storage-uri ''
```

### 3. Verify

Publish a fresh skill into the residency-tagged tenant; confirm the
bundle bytes land in the override bucket:

```bash
skill-pool publish ./acme-skill --version 1.0.0
aws s3 ls s3://skill-pool-acme-eu/<TENANT_UUID>/acme-skill/
```

For an end-to-end automated check, the integration test
`server/tests/data_residency.rs` verifies that two tenants with
different `storage_uri` values get their bundles in the right places
with zero cross-tenant leakage.

## What changes vs the default deploy

| Concern                          | Default deploy        | With per-tenant override                 |
|----------------------------------|-----------------------|------------------------------------------|
| Bundle reads                     | Global backend        | Override backend                         |
| Bundle writes                    | Global backend        | Override backend                         |
| Postgres rows                    | Shared DB             | Still shared                             |
| Audit events                     | Shared DB             | Still shared                             |
| `/v1/healthz` storage probe      | Probes global         | Still probes global only                 |
| Bundle key shape                 | `{tenant_id}/...`     | Unchanged — tenant_id-prefixed by design |
| Tenant resolution                | Subdomain / header    | Unchanged                                |
| Storage backend cached per       | Process               | Per tenant, lazy-built on first use      |

## Limitations + caveats

- **The Postgres row is not moved.** `tenants`, `skills`, `audit_events`,
  etc. still live in the shared DB. If your compliance regime requires
  the **metadata** to leave the region too, use
  [dedicated mode](./dedicated-mode.md) (separate DB per tenant) and
  stop here.
- **Healthz doesn't probe per-tenant backends.** The `/v1/healthz`
  storage probe only touches the default backend, so a misconfigured
  per-tenant URI won't surface there until the first bundle operation
  for that tenant fails. The `admin tenant-residency` command's
  pre-write validation catches malformed URIs (parsing only — DNS /
  credentials are checked on first use).
- **Cache is process-lifetime.** When you change a tenant's
  `storage_uri` the existing app processes keep using the old backend
  until restart. For multi-replica deploys do a rolling restart. A
  TTL-based cache or an admin-emitted invalidation is on the backlog
  (see `state::AppState::invalidate_tenant_storage`).
- **Bundle migration between backends is operator work.** If a tenant
  already has bundles in the global backend and you set a new
  `storage_uri`, future writes go to the override but old reads will
  fail — you must `aws s3 sync s3://default/<tenant_id>/
  s3://override/<tenant_id>/` before flipping. The bundle key shape
  (`{tenant_id}/{slug}/{version}.tar.gz`) is unchanged across
  backends; same UUIDs, same paths, just different roots.
- **No data-residency enforcement, only opt-in.** The server doesn't
  refuse writes from a tenant's region if their `storage_uri` points
  somewhere else. The tag is metadata; the URI is the enforcement.

## When to use what

- **Default deploy, no override** — every tenant on the global bucket.
  Free / Team tier. ~all customers.
- **`region` tag set, `storage_uri` unset** — metadata only. Useful for
  audit/compliance reporting; doesn't change where bundles live.
- **`storage_uri` set, `region` optional** — bundles physically move
  to the regional bucket. Enterprise tier. Pair with a region-local
  reverse proxy if you also want request termination to stay regional.
- **Dedicated mode (separate process)** — see
  [`dedicated-mode.md`](./dedicated-mode.md). Use when the tenant's
  *metadata* must also be region-local. More operational overhead
  (separate DB, separate host) — only do this when required.

## Related files

- `server/migrations/0018_tenant_data_residency.sql` — schema
- `server/src/state.rs::AppState::storage_for` — per-tenant resolver
- `server/src/admin.rs::set_tenant_residency` — admin helper
- `server/tests/data_residency.rs` — end-to-end test
- `packaging/bucket-policy/` — bucket + IAM templates
- `docs/enterprise/dedicated-mode.md` — when you need the bigger hammer
