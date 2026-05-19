# Migration: shared → dedicated

> Playbook for lifting one tenant out of a shared deploy and onto its
> own dedicated-mode instance, preserving `tenant_id` UUIDs so existing
> bundle keys keep working.

This is an operator-driven, planned migration. There is no online
"failover" today — expect a brief read-only window while you cut over.

## When to migrate a tenant out of shared

You typically migrate when one of the dedicated-mode triggers from
[`docs/enterprise/dedicated-mode.md`](./dedicated-mode.md#when-to-use-it)
becomes true for an existing shared tenant:

- New compliance / data-residency requirement (move tenant data into a
  specific region or jurisdiction).
- The tenant's volume is becoming a noisy neighbour.
- The customer is asking for an air-gapped, customer-managed copy.

If none of those apply, stay on shared. The migration has real cost
(downtime window, two systems to manage during the cutover) and the
shared deploy is designed for tenant isolation.

## Pre-migration checklist

- [ ] Tenant slug + UUID recorded. Get the UUID from the shared DB:
      ```sql
      SELECT id, slug FROM tenants WHERE slug = 'acme';
      ```
      You will preserve this UUID on the dedicated side.
- [ ] Dedicated Postgres provisioned. Same major version as the shared
      DB (Postgres 17 is the project default). Schema does **not** need
      to be pre-migrated; the server runs `sqlx::migrate!` on first start.
- [ ] Dedicated bundle store provisioned and policy applied. See
      [`packaging/bucket-policy/README.md`](../../packaging/bucket-policy/README.md#dedicated-buckets--applying-per-tenant).
- [ ] Dedicated host has a DNS record (e.g. `acme-skill-pool.example.com`)
      with TLS issued, behind your reverse proxy.
- [ ] Maintenance window scheduled. Plan for ~15–60 min of read-only or
      brief downtime depending on bundle volume; the actual cut is fast,
      but DNS propagation can drag.
- [ ] Communicated to the tenant: a) the window, b) the new hostname, c)
      that existing API tokens **will continue to work** (they're keyed
      on `tenant_id`, which we preserve).

## Step-by-step

### 1. Snapshot the shared DB filtered to one tenant

`pg_dump --table=... --where=...` does not exist with that exact spelling
in Postgres — `pg_dump` only supports `--table` selection without
row-level filtering. Two viable strategies:

**Option A (recommended): per-table `COPY ... WHERE` via a shell loop.**
Reliable, scriptable, restorable with `\copy` or `COPY ... FROM`.

```bash
# Tables that carry tenant_id directly. The set is enumerable from
# the migrations (grep "tenant_id" server/migrations/*.sql).
TABLES=(
  tenants
  tenant_users
  tenant_api_tokens
  skills
  audit_events
  # ...plus every other table with a tenant_id column. See the
  # migrations directory for the full list.
)

TENANT_ID="11111111-2222-3333-4444-555555555555"

mkdir -p ./export
for t in "${TABLES[@]}"; do
  if [[ "$t" == "tenants" ]]; then
    # Special case: tenants is keyed on id, not tenant_id.
    psql "$SHARED_DSN" -c "\COPY (SELECT * FROM tenants WHERE id = '$TENANT_ID') TO './export/${t}.csv' WITH CSV HEADER"
  else
    psql "$SHARED_DSN" -c "\COPY (SELECT * FROM $t WHERE tenant_id = '$TENANT_ID') TO './export/${t}.csv' WITH CSV HEADER"
  fi
done
```

For tables that reference tenant-owned rows transitively (e.g. a
`skill_dependencies` table keyed on `skill_id`, where `skills` is
tenant-scoped), join through the parent in the WHERE clause.

**Option B: `pg_dump --data-only` of the whole DB, then surgically `psql`
edit on restore.** Faster to write but much harder to verify. Only do
this if Option A's row counts give you a clear signal something is wrong
with the per-table approach.

Whichever you pick: **preserve the tenant `id` (UUID)**. The dedicated
deploy will resolve `tenancyTenantSlug = "acme"` against a row whose
`id` matches what's in the shared DB. Bundle keys are
`{tenant_id}/{slug}/{version}.tar.gz` — change the UUID and every bundle
URL breaks.

### 2. Bundle export

Bundle keys are namespaced by `tenant_id`, so a one-way sync of the
tenant's prefix copies exactly that tenant's bundles:

```bash
# Same-cloud (S3 → S3):
aws s3 sync \
  s3://skill-pool-prod-shared/${TENANT_ID}/ \
  s3://skill-pool-prod-acme/${TENANT_ID}/

# Cross-cloud (e.g. shared on S3, dedicated on GCS):
rclone sync \
  s3-shared:skill-pool-prod-shared/${TENANT_ID}/ \
  gcs-dedicated:skill-pool-prod-acme/${TENANT_ID}/
```

Verify the byte count matches before proceeding:

```bash
aws s3 ls --summarize --human-readable --recursive \
  s3://skill-pool-prod-shared/${TENANT_ID}/ | tail -2

aws s3 ls --summarize --human-readable --recursive \
  s3://skill-pool-prod-acme/${TENANT_ID}/ | tail -2
```

Counts and total size must match. If they don't, re-run `s3 sync` —
it's idempotent.

### 3. Provision the dedicated instance

Stand the dedicated host up but do **not** point DNS at it yet. Follow
either:

- [`docs/deploy/nixos.md`](../deploy/nixos.md) for a NixOS box, or
- [`docs/deploy/single-node.md`](../deploy/single-node.md) for a generic
  Docker/systemd box.

Configure with the dedicated env vars from
[`docs/enterprise/dedicated-mode.md`](./dedicated-mode.md#env-vars).
Leave `SKILL_POOL_TENANCY_MODE__MODE` unset (or `shared`) for the import
step — `dedicated` mode pins the tenant before any rows exist, which
will 401 the admin tooling. Flip it to `dedicated` after import (step 6).

Boot the dedicated server once so it runs migrations and creates an
empty schema. Then shut it down for the import.

### 4. Import into the dedicated DB

```bash
# Per the per-table COPY strategy from step 1:
for t in "${TABLES[@]}"; do
  psql "$DEDICATED_DSN" -c "\COPY $t FROM './export/${t}.csv' WITH CSV HEADER"
done

# Verify row counts match between shared and dedicated for each table.
for t in "${TABLES[@]}"; do
  shared=$(psql "$SHARED_DSN" -tAc "SELECT count(*) FROM $t WHERE tenant_id = '$TENANT_ID' OR (id = '$TENANT_ID' AND '$t' = 'tenants')")
  dedicated=$(psql "$DEDICATED_DSN" -tAc "SELECT count(*) FROM $t")
  printf "%-30s shared=%s dedicated=%s\n" "$t" "$shared" "$dedicated"
done
```

If any row count differs, stop. The most common cause is a foreign-key
ordering problem in the loop (import `tenants` first, then everything
else). Order the table list with `tenants` first and tables that depend
on `skills` after `skills`.

### 5. Cutover

1. Put the shared deploy into read-only mode for this tenant. There is
   no built-in feature flag for this today; the practical lever is to
   temporarily revoke the tenant's write-scoped tokens, or front the
   shared deploy with a reverse-proxy rule that returns 503 for write
   methods on this tenant's subdomain.
2. Run one final delta sync (step 2's `s3 sync` is idempotent; step 1's
   COPY is not — if writes happened between the snapshot and now, you'll
   need a delta export).
3. Flip the dedicated server's tenancy mode to `dedicated` and restart
   (or `nixos-rebuild switch` if you're on the NixOS module).
4. Run the dedicated-mode smoke from
   [`docs/enterprise/dedicated-mode.md`](./dedicated-mode.md#smoke-test).
5. Flip DNS: `acme.skill-pool.example.com` → CNAME →
   `acme-skill-pool.example.com` (or change the A record). TTL should
   already be low if you scheduled this properly.

### 6. Verify

- `curl /v1/healthz` on the dedicated host returns `{"status":"ok"}`.
- `curl -H "Authorization: Bearer $ACME_TOKEN" /v1/skills` on the
  dedicated host returns the same catalog the tenant saw on the shared
  deploy. Spot-check 2–3 skill slugs that the tenant uses heavily.
- `curl /v1/skills/<slug>/bundle.tar.gz` downloads a real bundle — this
  exercises the bundle-key path end-to-end, proving the UUID was
  preserved across the migration.
- Check the tenant's audit-event count is approximately preserved.

### 7. Decommission on the shared side

```bash
# Interactive (prompts you to retype the slug as confirmation)
skill-pool-server admin tenant-delete --slug acme

# Scripted (skip prompt — only safe in automation that already validated input)
skill-pool-server admin tenant-delete --slug acme --confirm
```

This issues a single `DELETE FROM tenants WHERE id = ...`; every business
table references `tenants(id) ON DELETE CASCADE` so all rows
(`skills`, `skill_drafts`, `skill_usage_events`, `tenant_api_tokens`,
`tenant_users`, `tenant_theme`, `tenant_oidc`, `tenant_saml`,
`tenant_role_mappings`, `tenant_stack_mappings`, `skill_dependencies`,
`audit_events`, …) are removed in the same transaction.

**Audit-event caveat:** `audit_events` cascades too. If your compliance
regime requires retaining audit history past the tenant lifetime, run
the SIEM export (`docs/enterprise/sso.md` covers the per-tenant SIEM
webhook) *before* the delete.

**Bundle storage is NOT swept** by the command — by design, since the
sweep semantics differ per backend (and the bundles may have forensic
value). The command prints the prefix you need to clean up:

```text
tenant deleted
  id:   11111111-2222-3333-4444-555555555555
  slug: acme

Bundle storage was NOT swept. To reclaim space, run:
  # fs://    rm -rf <storage_root>/11111111-2222-3333-4444-555555555555
  # s3://    aws s3 rm s3://<bucket>/11111111-2222-3333-4444-555555555555/ --recursive
```

If you would rather mark the tenant suspended (reversible) than delete
(irreversible), the manual SQL path is still available:

```sql
UPDATE tenants SET status = 'suspended' WHERE slug = 'acme';
```

## Pitfalls

- **Preserve UUIDs, don't regenerate.** Bundle keys are
  `{tenant_id}/...`. Generating a new tenant row on the dedicated side
  with a fresh UUID breaks every bundle URL the tenant has ever published
  unless you also rewrite every key in the bucket — which you should not
  do.
- **API tokens.** Tokens are stored as `(tenant_id, hashed_token, scope)`.
  Copying the `tenant_api_tokens` rows preserves existing tokens unchanged
  — the tenant's CI / CLI configs keep working without re-issuing.
- **Audit history.** `audit_events` rows carry `tenant_id`. Copying them
  preserves the trail; if you skip the audit table to keep the dedicated
  DB lean, document the gap (compliance audits will notice).
- **SSO config.** SAML / OIDC settings are tenant-scoped; copy
  `tenant_sso`, `tenant_saml`, and any group → role mapping tables. The
  IdP-side configuration (the actual SAML metadata exchange) does NOT
  need to change if the SP entity ID hasn't changed.
- **Replication lag during dump.** If you use logical replication or a
  read replica for the snapshot source, make sure the replica has caught
  up before the COPY. A stale read replica can drop the last few minutes
  of audit events / publishes.
- **Bundle store SSE keys.** If the shared bucket uses SSE-KMS with a
  key the dedicated IAM role can't decrypt, the bundles in the new
  bucket will be unreadable. Re-encrypt with the dedicated KMS key
  during the `s3 sync` step (`--sse aws:kms --sse-kms-key-id ...`).

## Related docs

- [`docs/enterprise/dedicated-mode.md`](./dedicated-mode.md) — the deploy
  target this migration produces.
- [`docs/tenancy.md`](../tenancy.md) — what `tenant_id` does and where
  it appears.
- [`docs/deploy/single-node.md`](../deploy/single-node.md),
  [`docs/deploy/nixos.md`](../deploy/nixos.md) — how to stand up the
  dedicated host.
- [`docs/ops/rollback.md`](../ops/rollback.md) — DR procedures; the
  bundle-versioning notes there apply to the migration too.
- [`packaging/bucket-policy/README.md`](../../packaging/bucket-policy/README.md) —
  bucket policy for the dedicated bundle store.
