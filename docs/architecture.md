# Architecture

> Phase 1 baseline. Updated as implementation progresses.

## Components

```
                              ┌─────────────────────┐
   developer's machine        │   skill-pool-web    │  (Phase 2)
   ┌────────────────────┐     │   per-tenant brand  │
   │  skill-pool CLI    │     └──────────┬──────────┘
   │  (Rust binary)     │                │ HTTPS (session cookies)
   └─────────┬──────────┘                │
             │ HTTPS, Bearer token       │
             │ subdomain or              │
             │ X-Skill-Pool-Tenant       │
             ▼                            ▼
   ┌────────────────────────────────────────────────┐
   │            skill-pool-server (Rust)            │
   │ ┌──────────────────────────────────────────┐   │
   │ │ Axum routes  →  tenant + auth extractors │   │
   │ └──────────────────────────────────────────┘   │
   │ ┌────────────┐ ┌────────────┐ ┌────────────┐   │
   │ │ Postgres   │ │ Object     │ │ Redis      │   │
   │ │ (metadata, │ │ storage    │ │ (cache /   │   │
   │ │  audit)    │ │ (bundles)  │ │  queue,    │   │
   │ │            │ │            │ │  Phase 2+) │   │
   │ └────────────┘ └────────────┘ └────────────┘   │
   └────────────────────────────────────────────────┘
                       │
                       ▼
              ┌────────────────────┐
              │  Git mirror repo   │  (Phase 1: optional;
              │  (skills source    │   Phase 5: bidirectional)
              │   of truth)        │
              └────────────────────┘
```

## Process boundaries

- **CLI** runs on every developer machine; symlinks skills into `~/.claude/skills/` or `<project>/.claude/skills/`.
- **Server** is stateless. All shared state lives in Postgres, object storage, and (Phase 2+) Redis.
- **Capturer daemon** (Phase 4) runs per-user as a systemd unit; talks to the server like the CLI does.

## Tenancy

- **Shared mode (default):** one server / DB / bucket; every row carries `tenant_id`; subdomain routing.
- **Dedicated mode (Enterprise opt-in):** one server / DB / bucket per tenant; same image, different DSN.

See `docs/tenancy.md`.

## Data flow — publish

```
CLI: skill-pool publish ./my-skill/
  → tar+gzip the directory                  (client)
  → POST /v1/skills (multipart)             (HTTPS)
    → tenant + auth extraction              (server)
    → bundle.tar.gz lint + secret scan      (server)
    → SHA-256 + upload to object storage    (server → opendal)
    → INSERT INTO skills (...)              (server → Postgres)
    → INSERT INTO audit_events (...)        (server → Postgres)
  ← 201 Created with canonical metadata     (server → CLI)
```

## Data flow — install

```
CLI: skill-pool ensure
  → load .skill-pool/manifest.toml
  → for each skill not yet in ~/.skill-pool/library/<tenant>/<slug>@<ver>/:
      GET /v1/skills/{slug}/bundle.tar.gz   (HTTPS; redirect to signed URL on S3)
      extract to library
  → symlink library entry into .claude/skills/
```

## Key invariants

1. Every business query filters by `tenant_id`. Code review + tests enforce.
2. Object storage keys are tenant-prefixed (`{tenant_id}/...`). Bundle URIs in DB are opaque to the client.
3. Audit log writes are non-optional. Every mutating endpoint writes.
4. App is stateless; safe to replicate horizontally.
