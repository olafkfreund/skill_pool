# Changelog

All notable changes to this project. Format roughly follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions track the
git tags published from `main`.

## [0.3.4] — 2026-05-25

### Fixed

- **Shallow pack must include blobs shared with parent** (#67, follow-up
  to #58) — `revwalk.hide(parent)` was hiding the parent's blobs too,
  dropping any blob the tip's tree referenced from a shared (unchanged)
  file. Replaced shallow's pack-build path with explicit
  `collect_commits_up_to_depth` + `pb.insert_commit` per commit;
  full-clone path unchanged. Regression test pins the shared-blob case
  end-to-end via in-memory bare repo.

## [0.3.3] — 2026-05-25

### Fixed

- **Two-phase deepening for smart-HTTP stateless-RPC** (#66, follow-up
  to #58) — git's smart-HTTP client makes TWO POSTs for shallow clones;
  the first expects only the shallow section. v0.3.2 emitted full
  shallow+NAK+pack on both POSTs, corrupting the client. Now returns
  only the shallow section when `deepen` is set but `done` is unset.

## [0.3.2] — 2026-05-25

### Fixed

- **Shallow boundary emission for root commits** (#65, follow-up to
  #58) — `compute_shallow_boundaries` was skipping commits with no
  parents as an optimization; git's protocol requires every boundary
  to be emitted, including root commits. Fix emits unconditionally.

## [0.3.1] — 2026-05-25

### Fixed

- **CLI plugin publish wire shape** (#57) — `skill-pool plugin publish` now
  POSTs the `PublishBody` envelope (`{slug, manifest, contents, sourcing_mode,
  status}`) the server expects, not the bare `PluginManifest`. Real-world
  publishes previously returned 400. Regression test pins the envelope shape
  via wiremock body matcher.
- **plugin_git shallow clone** (#58) — Server now advertises `shallow` in its
  capability list and handles `shallow <sha>` / `deepen <n>` upload-pack lines.
  `git clone --depth=1` and `claude plugin install` against the marketplace
  source URL no longer fail with "Server does not support shallow clients". 7
  new unit tests cover capability advertisement, parser handling, and shallow
  boundary computation across depth=1 / depth=2 / walks-past-root.
- **In-process queue fallback** (#59) — `POST /v1/plugins/import` no longer
  hard-requires Redis. When `SKILL_POOL_REDIS_URL` is unset, the import handler
  spawns a tokio task running `run_mirror` directly and returns
  `outcome:"enqueued_inline"` + `job_id:"inline-<plugin_id>"`. No durability
  across restarts — operators who need that should provision Redis.

## [0.3.0] — 2026-05-24

The plugins-and-marketplace release.

### Added

- **Plugin schema** (#29) — `plugins`, `plugin_contents`,
  `plugin_marketplace_entries` tables with tenant-isolation FKs.
- **Plugin REST API** (#30) — `POST /v1/plugins`, `GET /v1/plugins`,
  versioning, archive. RBAC: curators/admins publish, all roles read.
- **Marketplace endpoints** (#31) — per-tenant
  `/.claude-plugin/marketplace.json` plus per-plugin dumb-HTTP git endpoints
  under `/git/plugins/<slug>.git/`. Public read, no auth.
- **Mirror background worker** (#32) — `POST /v1/plugins/import` clones an
  external plugin repo into local storage, parses its manifest, indexes the
  contents, and refreshes on a configurable interval (default 24h).
- **CLI plugin subcommands** (#33) — `skill-pool plugin
  publish|list|add|import|marketplace-url`.
- **Web admin Plugins surfaces** (#34) — list / new (composer) / detail /
  import pages under `/admin/plugins`. Role-gated.
- **Public marketplace browser** (#35) — `/marketplace` and
  `/marketplace/[slug]` with copy-to-clipboard install command, no auth.
- **Project + bootstrap plugin resolution** (#36) — project manifests can
  pin `[[plugins]]`; `ensure` resolves transitively and dedupes against
  direct entries.
- **Docs** (#37 wave 1+2, merged via #39 and #56) — `docs/plugins.md`
  overview, `docs/plugin-manifest-schema.md` reference,
  `docs/wiki/Plugin-Authoring.md` walkthrough, REST + CLI reference updates,
  plugin storage operator guide, mermaid architecture diagram, README
  feature-list line.
- **E2E acceptance gate** (#38) — `scripts/seed-demo-plugin.sh` seeds a
  sample plugin; `docs/e2e/plugin-install-2026-05.md` captures the live run
  against the deployed portal.

### Fixed

- **Seeder PGPORT honored** (#46) — `scripts/import-skills.sh` and
  `scripts/seed-tenant.sh` now respect `$PGPORT` for non-default Postgres
  deployments.

### Known limitations at ship (all resolved in v0.3.1 — v0.3.4)

- CLI publish payload mismatch (#57) → v0.3.1 (#61).
- Git shallow clone unsupported (#58) → initial fix v0.3.1 (#62);
  downstream protocol bugs caught by live re-test fixed in v0.3.2 (#65),
  v0.3.3 (#66), and v0.3.4 (#67).
- Redis required for `/v1/plugins/import` (#59) → in-process fallback added in
  v0.3.1 (#63).

## [0.2.2] — 2026-05-21

- Internal release plumbing: correct `npmDepsHash` and bump web package.

## [0.2.1] — 2026-05-20

- Refreshed `npmDepsHash` after web dep tree changes.

## [0.2.0] — 2026-05-20

- Vitest coverage for Projects + Plans editors.

[0.3.4]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.3.4
[0.3.3]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.3.3
[0.3.2]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.3.2
[0.3.1]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.3.1
[0.3.0]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.3.0
[0.2.2]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.2.2
[0.2.1]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.2.1
[0.2.0]: https://github.com/olafkfreund/skill_pool/releases/tag/v0.2.0
