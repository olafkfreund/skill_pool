# Capacity planning

## What this document gives you

Three deployment tiers — homelab (≤25 developers), team/startup (25–200
developers), and enterprise (1k+ developers across many tenants) — with
concrete CPU, RAM, disk, pool, and replica numbers for each. The tables
are derived analytically from the schema (`server/migrations/`), the
process baseline implied by `packaging/systemd/skill-pool-server.service`
(`MemoryMax=2G`, `TasksMax=1024`), and the bundle-delivery contract in
`server/src/routes/skills.rs::get_bundle`. They are not yet backed by a
production load test: the "100 RPS / p95 < 100 ms on 2 vCPU" target from
issue #10 is a goal, not a measurement. Treat every number here as a
starting point for capacity planning, not a guarantee.

## Sizing variables

The numbers below are driven by a small set of inputs. Hold the rest
constant and these are what move the needle.

- **Tenant count** — drives row counts in `tenants`, `tenant_users`,
  `tenant_api_tokens`, and the per-tenant index branches.
- **Active concurrent developer count** — the real load driver. Far
  smaller than total developer headcount: a 200-dev team typically has
  10–30 concurrent CLI sessions.
- **Catalog size** — skills × versions × kinds. Each
  `(tenant_id, slug, version)` is a row in `skills`; `kind` (added in
  migration `0015_catalog_item_kind`) is part of the dedup key.
- **Bundle size** — typical SKILL.md tar.gz: a `frontmatter + markdown`
  bundle is usually 5–20 KB. Bundles with embedded assets (images,
  examples) can reach 100 KB–1 MB; cap is enforced by the ingress proxy
  (`nginx.ingress.kubernetes.io/proxy-body-size: 50m` in
  `docs/deploy/kubernetes.md`).
- **Embeddings enabled** — when the server is built `--features fastembed`
  and `SKILL_POOL_EMBEDDING__ENABLED=true`, the BGE-small-en-v1.5 model
  is lazy-loaded into process memory on first dedup request (see
  `server/src/embedding.rs`). Adds resident RAM, model download
  bandwidth on first use, and pgvector index pressure (HNSW on
  `skills.description_embedding`).
- **OTLP exporter** — `--features otlp` plus
  `OTEL_EXPORTER_OTLP_ENDPOINT` adds a background span-shipping task and
  ~10–30 MB resident for the batched span queue.
- **Storage backend** — `fs://` streams bundle bytes through the app
  process; S3/GCS/Azure issue a 307 redirect to a 5-minute presigned
  URL (`PRESIGN_TTL` in `server/src/storage.rs`, returned by
  `presign_read`). App bandwidth changes by orders of magnitude between
  the two.

## Tier 1 — Homelab / small team (1–25 developers)

| Resource          | Value                                              |
| ----------------- | -------------------------------------------------- |
| CPU               | 1 vCPU                                             |
| RAM               | 2 GB                                               |
| Disk (PG data)    | 5 GB                                               |
| Disk (bundles)    | 1 GB                                               |
| Postgres          | Same host, in-process or container, `postgresql-16` + `pgvector` |
| sqlx pool size    | 20 (hardcoded in `server/src/state.rs`)            |
| App replicas      | 1                                                  |
| Network bandwidth | 10 Mbps sustained, 100 Mbps burst                  |
| Deploy path       | `docs/deploy/single-node.md` (systemd + Caddy)     |

Assumption: storage backend is `fs://` (the
`default_storage_uri` from `server/src/config.rs`). RAM target fits
inside the unit's `MemoryMax=2G` headroom.

**Worked example.** 20 developers, 200 skills × 3 versions × 2 kinds
(skill+agent) = 1,200 catalog rows, fs:// storage on a 1-vCPU / 2 GB
VPS. Bundle storage at 15 KB/row = ~18 MB. Database at ~1 KB/row +
indexes ≈ 5 MB. Concurrent download peak of 3 developers × 20 KB
bundles = 60 KB/s app egress. Runs comfortably; the `MemoryMax=2G`
ceiling is the binding constraint, and embeddings should be left off.

## Tier 2 — Team / startup (25–200 developers, 1–5 tenants)

| Resource          | Value                                              |
| ----------------- | -------------------------------------------------- |
| CPU               | 2 vCPU per app replica                             |
| RAM               | 2 GB per app replica (4 GB if embeddings enabled)  |
| Disk (PG data)    | 50 GB                                              |
| Disk (bundles)    | S3 bucket, no local disk on app hosts              |
| Postgres          | Managed (RDS `db.t3.medium` class: 2 vCPU, 4 GB)   |
| sqlx pool size    | 20 (still hardcoded; see "Per-resource sizing")    |
| App replicas      | 2 (active/active behind reverse proxy)             |
| Network bandwidth | 50 Mbps sustained per replica                      |
| Deploy path       | `docs/deploy/nixos.md` on two boxes, or `docs/deploy/kubernetes.md` light |

Assumption: bundles live on S3 via `SKILL_POOL_STORAGE_URI=s3://...`
(scheme parsed by `Storage::from_uri` in `server/src/storage.rs`). The
`get_bundle` path returns a 307 redirect to a presigned URL, so app
egress per download collapses to a few hundred bytes of HTTP headers.

**Worked example.** 150 developers, 1,500 skills × 4 versions × 2 kinds
= 12,000 catalog rows, fastembed enabled, S3 storage, RDS `db.t3.medium`.

- App RAM per replica: ~150 MB process baseline + ~130 MB fastembed
  resident (BGE-small-en-v1.5 ONNX session, see caveat below) + ~50 MB
  tokio task arenas and connection buffers = ~330 MB. Fits 2 GB
  comfortably; 4 GB recommended for embedding-heavy bursts.
- App CPU per replica: at the issue #10 acceptance target of 100 RPS
  p95 < 100 ms on 2 vCPU, this is well within budget.
- DB rows: 12,000 catalog + ~200 KB of audit/day + usage events at
  ~30 downloads/min × 86,400 min/month = ~2.6 M events/month.

## Tier 3 — Enterprise / multi-tenant (1k+ developers, hundreds of tenants)

| Resource              | Value                                          |
| --------------------- | ---------------------------------------------- |
| CPU                   | 2 vCPU per pod                                 |
| RAM                   | 1 GB request / 2 GB limit per pod (matches `docs/deploy/kubernetes.md` `resources.limits`) |
| Disk (PG data)        | 500 GB (writer) + read replica                 |
| Disk (bundles)        | S3 per region, CDN in front                    |
| Postgres              | Managed primary + read replica (`SKILL_POOL_DATABASE_READ_URL`)        |
| sqlx pool size        | `SKILL_POOL_DB_POOL_SIZE` (default 20); 50–100 typical for this tier  |
| App replicas          | HPA `minReplicas=2`, `maxReplicas=20`, CPU target 70% (from `docs/deploy/kubernetes.md`) |
| Network bandwidth     | 1 Gbps cluster ingress, CDN absorbs bundle egress |
| Deploy path           | `docs/deploy/kubernetes.md` + per-region buckets (templates: `packaging/bucket-policy/`) |

Assumption: bundles are fronted by a CDN (CloudFront / Cloudflare /
Fastly) configured to follow the 307 redirect from `get_bundle` and
cache at the edge. Without a CDN the redirect target is hit on every
request, which is still cheaper than proxy-bytes but loses edge
locality.

**Worked example.** 5,000 developers across 50 tenants, 10,000 skills
× 5 versions × 2 kinds = 100,000 catalog rows.

- Catalog rows in `skills`: 100,000 × ~1 KB metadata = ~100 MB.
- With embeddings: + 100,000 × 1,536 B (384-dim f32 vector) = ~150 MB
  in column data, plus the HNSW index (`idx_skills_description_embedding_hnsw`
  in migration `0009_embeddings`) at roughly the same order again ≈
  ~300 MB total pgvector footprint.
- Usage events: 5,000 devs × 10 downloads/day × 365 days × 64 B ≈
  ~1.2 GB/year before retention trimming.
- Pod count: at 100 RPS per pod × 70% target utilisation, 20 max pods
  buys ~1,400 RPS headroom — sufficient for 5,000 devs at typical CLI
  burst patterns.

## Per-resource sizing maths

### Database storage

Per-row footprint, derived from column types in
`server/migrations/0001_init.sql` and follow-up migrations:

| Table                  | Row size (no embedding) | Row size (with embedding) |
| ---------------------- | ----------------------- | ------------------------- |
| `tenants`              | ~256 B                  | n/a                       |
| `users`                | ~256 B                  | n/a                       |
| `tenant_users`         | ~64 B                   | n/a                       |
| `tenant_api_tokens`    | ~256 B                  | n/a                       |
| `skills`               | ~1 KB                   | ~2.5 KB                   |
| `skill_drafts`         | ~1 KB                   | ~2.5 KB                   |
| `audit_events`         | ~256 B                  | n/a                       |
| `skill_usage_events`   | ~64 B                   | n/a                       |

`+1.5 KB` for the embedding column is exactly `384 dims × 4 B/dim`
(f32) as set by migration `0009_embeddings`. Indexes add roughly
20–40% on top of the raw row sizes for the B-tree and GIN indexes
named in the same migrations.

Worked example: 10 tenants × 200 skills × 5 versions × 2 KB
(with embeddings) ≈ 20 MB of `skills` data, plus ~5 MB of indexes ≈
25 MB total. Usage events at 10 RPS download × 86,400 sec/day × 365
day/year × 64 B/row ≈ 20 GB/year — apply a 90-day retention policy
to keep the table around 5 GB.

### Bundle storage

Per-bundle: SKILL.md is rarely > 50 KB; a typical bundle (frontmatter
+ markdown + a small assets directory) is 5–20 KB. Bundles are
immutable once published (see "Backup" in `docs/deploy/single-node.md`),
so storage grows monotonically with catalog size.

| Catalog rows | Avg bundle | Total bundle storage |
| ------------ | ---------- | -------------------- |
| 1,000        | 15 KB      | ~15 MB               |
| 10,000       | 15 KB      | ~150 MB              |
| 100,000      | 20 KB      | ~2 GB                |

### RAM

Per-process resident, approximate:

| Component                         | Resident size                    |
| --------------------------------- | -------------------------------- |
| idle Rust binary + tokio runtime  | ~100 MB                          |
| sqlx pool of 20 connections       | ~20 MB (1 MB/conn buffer + state)|
| fastembed model loaded            | ~130 MB (bge-small-en-v1.5)      |
| OTLP exporter queue               | ~10–30 MB                        |
| per in-flight bundle (fs://)      | bundle size × concurrent requests|

The single-node systemd unit caps total resident at 2 GB
(`MemoryMax=2G` in `packaging/systemd/skill-pool-server.service`),
which leaves ~1.7 GB for in-flight bundles after baseline + fastembed.

### Pool sizing

The classic Little's Law form: `connections ≈ RPS × p95_seconds + buffer`.

| Sustained RPS | p95 (sec) | Needed pool | Today                 |
| ------------- | --------- | ----------- | --------------------- |
| 10            | 0.05      | 1 + 4 = 5   | 20 (4× headroom)      |
| 50            | 0.1       | 5 + 5 = 10  | 20 (2× headroom)      |
| 100           | 0.1       | 10 + 5 = 15 | 20 (1.3× headroom)    |
| 200           | 0.1       | 20 + 5 = 25 | 20 (saturated)        |

Anything sustained above ~150 RPS per replica will run the pool hot.
Bump it via `SKILL_POOL_DB_POOL_SIZE` (NixOS option: `dbPoolSize`).

### Network

App-side bandwidth depends entirely on the storage backend.

| Backend            | App bandwidth per download    |
| ------------------ | ----------------------------- |
| `fs://`            | bundle size (e.g. 15 KB)      |
| `s3://` (proxy)    | bundle size (`?bytes=true`)   |
| `s3://` (redirect) | ~500 B (307 + headers only)   |

At 100 downloads/sec on a 20 KB bundle, the `fs://` path needs
~16 Mbps; the redirect path needs ~400 Kbps. The `bytes=true` query
override (see `BundleQuery` in `server/src/routes/skills.rs`) forces
proxy-bytes mode for clients behind redirect-stripping proxies.

## What to scale first when traffic grows

Ordered cheapest to most invasive:

1. **Move bundles to S3.** Already supported via
   `SKILL_POOL_STORAGE_URI=s3://...`. Cuts app bandwidth by ~99% and
   takes ~1 hour of S3+IAM setup. Cuts compute too — the app stops
   touching bundle bytes on hot paths.
2. **Bump the sqlx pool size.** Set `SKILL_POOL_DB_POOL_SIZE` (or
   `services.skill-pool-server.dbPoolSize` on NixOS); the read pool
   (if configured) shares the same cap.
3. **Add a second app replica behind the proxy.** Stateless app means
   replicas just plug in; see the `replicas: 2` example in
   `docs/deploy/kubernetes.md`.
4. **Add a Postgres read replica.** Set `SKILL_POOL_DATABASE_READ_URL`
   to the replica DSN. Read-only handlers (catalog list, detail, deps,
   usage timelines, decay candidates) route there; writes stay on the
   primary.
5. **Cache theme and auth lookups in Redis.** Planned (issue #10 §A).
   Eliminates the per-request `tenants` + `tenant_users` joins.
6. **Shard tenants across dedicated deploys.** The "dedicated"
   `TenancyMode::Dedicated { tenant_slug }` from `server/src/config.rs`
   exists today; the operational pattern is one app+DB per large tenant.

## Known limits and honest caveats

- No production load-test data has been collected against the current
  codebase. The "100 RPS / p95 < 100 ms on 2 vCPU" target in issue #10
  is a goal, not a measurement.
- The fastembed resident size figure of ~130 MB is an estimate based on
  the BGE-small-en-v1.5 ONNX model weights (`server/src/embedding.rs`).
  Actual residency depends on ONNX runtime version, page-cache state,
  and the operator's batching pattern. Measure on the target host
  before committing pod limits.
- `SKILL_POOL_DATABASE_READ_URL` routes only the catalog read paths
  (list/detail/deps/usage timelines/decay candidates). Token validation
  and auth lookups always hit the primary so a misconfigured replica
  cannot lock anyone out.
- Bundle CDN behaviour is backend-dependent. The 307 redirect from
  `get_bundle` only fires for backends whose `presign_read` returns
  `Some` — `fs://` always streams bytes, S3 always presigns. Edge
  caching needs to be configured on the CDN side; the app does not set
  `Cache-Control` headers on the redirect target.
- `MemoryMax=2G` and `TasksMax=1024` in
  `packaging/systemd/skill-pool-server.service` are the documented
  single-node ceilings. Larger deployments raise them via systemd drop-ins
  or move to Kubernetes where the limits are set per pod.
