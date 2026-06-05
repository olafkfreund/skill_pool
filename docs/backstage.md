# Backstage integration

skill-pool is registered in the [Backstage](https://backstage.io/) Software
Catalog and ships its documentation as [TechDocs](https://backstage.io/docs/features/techdocs/)
built straight from this repository. This page describes the catalog model and
how the docs stay in sync.

## Catalog model

The catalog descriptor lives at [`catalog-info.yaml`](https://github.com/olafkfreund/skill_pool/blob/main/catalog-info.yaml)
in the repo root. It declares one `System` and the entities that make it up:

| Entity | Kind | Notes |
| --- | --- | --- |
| `skill-pool` | System | Umbrella; owns the TechDocs (`backstage.io/techdocs-ref: dir:.`), domain `public`. |
| `skill-pool-server` | Component (`service`) | Rust + Axum API; **provides** `skill-pool-api`; **depends on** the db, object store, and cache. |
| `skill-pool-web` | Component (`website`) | SvelteKit portal; **consumes** `skill-pool-api`. |
| `skill-pool-cli` | Component (`library`) | Rust CLI; **consumes** `skill-pool-api`. |
| `skill-pool-api` | API (`openapi`) | Definition tracked in [`openapi.yaml`](https://github.com/olafkfreund/skill_pool/blob/main/openapi.yaml). |
| `skill-pool-db` | Resource (`database`) | Postgres 17 + pgvector. |
| `skill-pool-object-store` | Resource (`object-storage`) | opendal bundle storage (S3/GCS/Azure/MinIO/fs). |
| `skill-pool-cache` | Resource (`cache`) | Optional Redis queue + cache. |

All entities are owned by `group:default/olafkfreund` and grouped under the
`skill-pool` system, so Backstage renders the full provides/consumes/depends-on
graph on the system page.

## How docs stay in sync

There are three moving parts, and together they make the sync hands-off:

1. **Registration + local builder.** Once `catalog-info.yaml` is registered as a
   Location, Backstage re-reads it on a schedule. Because the System carries
   `backstage.io/techdocs-ref: dir:.`, the TechDocs *local builder* rebuilds the
   site from [`mkdocs.yml`](https://github.com/olafkfreund/skill_pool/blob/main/mkdocs.yml)
   on `main` — no push from CI required.

2. **README → TechDocs home.** `README.md` is the single source of truth for the
   project overview. [`scripts/sync-techdocs.py`](https://github.com/olafkfreund/skill_pool/blob/main/scripts/sync-techdocs.py)
   regenerates `docs/index.md` from it, rewriting links for the TechDocs context
   (repo-relative `docs/...` links are stripped; links to code outside `docs/`
   become absolute GitHub URLs). Run it after editing the README:

   ```bash
   python3 scripts/sync-techdocs.py          # regenerate docs/index.md
   python3 scripts/sync-techdocs.py --check  # CI gate: fail if drifted
   ```

3. **CI gate + optional publish.** The
   [`TechDocs` workflow](https://github.com/olafkfreund/skill_pool/blob/main/.github/workflows/techdocs.yml)
   runs on every PR and on `main`: it checks the README↔index sync and does a
   real `mkdocs build`, so broken docs never reach `main`. An optional
   `publish` job pushes a pre-built site to an external TechDocs bucket (AWS S3)
   when the repo variable `TECHDOCS_PUBLISH=true` and the publisher settings are
   present; otherwise Backstage's local builder serves the docs directly.

## Building the docs locally

```bash
python3 -m venv .venv && . .venv/bin/activate
pip install mkdocs-techdocs-core
mkdocs serve          # live preview on http://127.0.0.1:8000
mkdocs build          # one-shot build into ./site
```

This mirrors exactly what the Backstage TechDocs builder runs.
