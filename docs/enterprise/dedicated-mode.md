# Dedicated-mode deploy

> Enterprise tier. One tenant per server instance, no subdomain routing,
> separate Postgres and bundle store. Same binary as shared-mode.

## What dedicated mode is

In **shared mode** (the default), one `skill-pool-server` process serves
many tenants. Each request carries its tenant identity through a Host
subdomain (`acme.skill-pool.example.com`) or, in dev, the
`X-Skill-Pool-Tenant` header. Every row in every business-data table
carries a `tenant_id`, every bundle key is prefixed by the tenant's UUID,
and every handler filters by the tenant resolved at the edge.

In **dedicated mode**, the operator pins a single tenant at startup. The
tenant extractor short-circuits — it returns the pinned slug regardless
of what the request looked like — so:

- There is no subdomain routing requirement. The instance can sit behind
  `acme-skill-pool.example.com` (or any host, including a bare IP).
- The `X-Skill-Pool-Tenant` header is ignored. Sending one does not
  switch tenants; you can't.
- The tenant isolation guarantees that defend a shared cluster from
  cross-tenant access are still in place — they just have nothing to do.

The deploy is otherwise identical: same Docker image, same migrations,
same admin CLI, same API surface.

The short-circuit lives in `server/src/tenant.rs::slug_from_request`:
when `state.tenancy()` is `TenancyMode::Dedicated { tenant_slug }` the
function returns the pinned slug without parsing the Host header or
looking for `X-Skill-Pool-Tenant`. The downstream extractor still
resolves that slug to a `tenant_id` against the `tenants` table — so the
slug must exist as an active row, just like in shared mode.

## When to use it

Pick dedicated mode when one of the following dominates the cost calculus:

1. **Compliance / data-residency.** The tenant's data must not share a
   Postgres instance or bundle store with anyone else, often for a
   specific region (EU, GovCloud, on-prem). Dedicated mode lets you put
   a tenant's everything in their region without forking the codebase.
2. **Very large customer.** A single tenant pushes enough load that
   noisy-neighbour effects matter, or their bundle volume swamps the
   shared bucket's lifecycle policies.
3. **Air-gapped / customer-managed.** The tenant runs their own copy on
   their own infrastructure (`skill-pool` self-hosted by the customer).
   No multi-tenant complexity ships to them.

Stay on shared mode if you only need logical isolation. The shared
deployment has been designed for it (row-level filtering, tenant-prefixed
bundle keys, tenant-scoped tokens) and it's strictly cheaper to operate.

## Configuration

### Env vars

```bash
# Required for dedicated mode.
SKILL_POOL_TENANCY_MODE__MODE=dedicated
SKILL_POOL_TENANCY_MODE__TENANT_SLUG=acme

# Standard env (same as any deploy):
SKILL_POOL_BIND=0.0.0.0:8080
SKILL_POOL_DATABASE_URL=postgres://skillpool@db.acme.internal/skillpool
SKILL_POOL_STORAGE_URI=s3://skill-pool-prod-acme?region=eu-west-1

# Optional in shared mode; effectively ignored in dedicated mode but
# harmless to leave set (the dedicated path never reads it):
SKILL_POOL_DEFAULT_TENANT=acme
```

The double-underscore separator (`__`) is how
[`figment`](https://docs.rs/figment) maps env var names onto nested
struct fields. `SKILL_POOL_TENANCY_MODE__MODE` becomes
`config.tenancy_mode.mode`. There is no flat `SKILL_POOL_TENANCY_MODE`
variable.

The `tenant_slug` must already exist as a row in the `tenants` table on
the dedicated Postgres. On a fresh deploy, run once after the first
migration:

```bash
skill-pool-server admin tenant-create --slug acme --name "Acme Corp" --plan enterprise
skill-pool-server admin token-create --tenant acme --name bootstrap
```

(The boot would otherwise return 401 for every request — the slug
resolves, but to no row.)

### NixOS option mapping

The `services.skill-pool-server` NixOS module exposes two options for the
tenancy mode:

| Option                 | Type                              | Default      | Env var                                  |
|------------------------|-----------------------------------|--------------|------------------------------------------|
| `tenancyMode`          | enum `"shared"` \| `"dedicated"`  | `"shared"`   | `SKILL_POOL_TENANCY_MODE__MODE`          |
| `tenancyTenantSlug`    | nullable string                   | `null`       | `SKILL_POOL_TENANCY_MODE__TENANT_SLUG`   |

Minimal dedicated config:

```nix
services.skill-pool-server = {
  enable = true;
  package = skill-pool.packages.${pkgs.system}.skill-pool-server;

  bind = "127.0.0.1:8080";
  databaseUrl = "postgres://skillpool@localhost/skillpool";
  storageUri = "s3://skill-pool-prod-acme?region=eu-west-1";

  tenancyMode = "dedicated";
  tenancyTenantSlug = "acme";

  environmentFile = config.age.secrets."skill-pool.env".path;
};
```

The module asserts at evaluation time that `tenancyTenantSlug` is set
when `tenancyMode = "dedicated"` — a missing slug fails the
`nixos-rebuild` instead of producing a server that 401s on every request.

## Infrastructure pattern

A dedicated-mode deploy is "one tenant's vertical stack":

```
                            HTTPS
       acme-skill-pool.example.com
                  |
                  v
        +---------------------+
        | Caddy / Nginx       |   (single virtualHost, no wildcard)
        | reverse proxy       |
        +---------------------+
                  |
                  v
        +---------------------+
        | skill-pool-server   |   tenancyMode = dedicated
        | (one process)       |   tenancyTenantSlug = acme
        +---------------------+
                  |
        +---------+---------+
        |                   |
        v                   v
   Postgres            S3 bucket
   skillpool DB        skill-pool-prod-acme
   (per-tenant)        (per-tenant)
```

### Postgres

Provision a Postgres instance dedicated to this tenant. The schema is the
same as a shared deploy — the server runs `sqlx::migrate!` on startup —
but the data plane is unshared. There is no `tenant_id` partitioning at
the DB level beyond what the rows already carry; the isolation comes from
no other tenant having credentials to this DB.

For data-residency: put the DB in the customer's region (`eu-west-1` for
an EU tenant, etc.) and reflect that in the bundle store choice.

### Bundle store (S3)

Pair dedicated mode with the **dedicated bucket policy**
([`packaging/bucket-policy/bucket-policy-dedicated.json`](../../packaging/bucket-policy/bucket-policy-dedicated.json)).
The bucket policy README walks through provisioning the bucket with the
right access block, versioning, and IAM statement
([`packaging/bucket-policy/README.md`](../../packaging/bucket-policy/README.md#dedicated-buckets--applying-per-tenant)).

Worth re-stating: bundle keys are still namespaced by `tenant_id` UUID
inside the bucket. With one tenant per bucket that namespacing is
redundant, but it keeps the storage code path identical between shared
and dedicated. Do not try to rewrite keys to drop the prefix.

### Reverse proxy

A single virtualHost. No wildcard. No tenant-aware routing rules:

```caddyfile
acme-skill-pool.example.com {
  @api path /v1/* /metrics
  reverse_proxy @api 127.0.0.1:8080
  reverse_proxy 127.0.0.1:3000
}
```

(The same Caddyfile works for the web UI on :3000 fronting the API on
:8080 — see [`docs/deploy/single-node.md`](../deploy/single-node.md).)

## Smoke test

After `nixos-rebuild switch` (or `systemctl restart skill-pool-server`):

```bash
# Health endpoint never required tenancy; verify the process is up.
curl -fsS http://127.0.0.1:8080/v1/healthz | jq

# The catalog endpoint requires a tenant in shared mode. In dedicated
# mode this works WITHOUT any tenant header — that's the whole point.
curl -fsS http://127.0.0.1:8080/v1/skills \
  -H "Authorization: Bearer $ACME_TOKEN"
```

In shared mode the same `curl` would 400 with
`tenant_resolution_failed`. A successful catalog response is proof the
dedicated short-circuit is wired.

The integration test
[`server/tests/dedicated_mode.rs`](../../server/tests/dedicated_mode.rs)
locks this behaviour down for the test suite.

## Limitations / non-goals

- **One tenant per instance.** Dedicated mode does not give you "two
  isolated tenants in the same process". For that, run two processes,
  each with its own pinned slug and its own DB. There is no in-process
  multiplexing.
- **Bucket policies are independent.** The shared and dedicated bucket
  policy templates in `packaging/bucket-policy/` are alternatives, not
  layers. A dedicated deploy paired with the *shared* bucket policy
  works (the storage code path doesn't care), but it gives up the
  defence-in-depth of "this IAM role can only see this tenant's
  bucket". Use the dedicated policy.
- **DB pool sizing is unchanged.** `SKILL_POOL_DB_POOL_SIZE` still caps
  the pool the same way; a dedicated tenant with high load needs its
  pool sized for *that tenant's* peak.
- **Audit + SIEM are still per-tenant.** Audit events still carry the
  pinned `tenant_id`; SIEM export is unchanged. There is no separate
  "dedicated-mode audit" pipeline.

## Related docs

- [`docs/tenancy.md`](../tenancy.md) — the canonical description of the
  tenant resolution algorithm.
- [`docs/deploy/single-node.md`](../deploy/single-node.md) — single-node
  deploy pattern. Pair with dedicated mode for a "boxed Enterprise"
  appliance.
- [`docs/deploy/nixos.md`](../deploy/nixos.md) — NixOS module reference.
- [`packaging/bucket-policy/README.md`](../../packaging/bucket-policy/README.md) —
  shared vs dedicated bucket layouts.
- [`docs/enterprise/migration-shared-to-dedicated.md`](./migration-shared-to-dedicated.md) —
  playbook for moving a tenant out of a shared deploy.
