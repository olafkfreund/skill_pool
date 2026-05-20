#!/usr/bin/env bash
# seed-tenant.sh — populate tenant-level state (users, theme, SSO, drafts,
# custom domain, stack mappings, usage events) for the local dev portal.
#
# Idempotent: every INSERT uses ON CONFLICT DO NOTHING (or DO UPDATE for
# rows we want to refresh on each run). Re-running produces no duplicates.
#
# Targets the `acme` tenant. Override with SKILL_POOL_TENANT.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

TENANT="${SKILL_POOL_TENANT:-acme}"
PG_ARGS=(-h 127.0.0.1 -p 55432 -U skillpool -d skillpool)
export PGPASSWORD="${PGPASSWORD:-skillpool}"

log() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

psql_run() { psql "${PG_ARGS[@]}" "$@"; }

resolve_tenant_id() {
  local tid
  tid="$(psql_run -tA -c "select id from tenants where slug='$TENANT'")"
  if [[ -z "$tid" ]]; then
    err "tenant '$TENANT' not found"
    exit 1
  fi
  printf '%s' "$tid"
}

main() {
  local tenant_id
  tenant_id="$(resolve_tenant_id)"
  log "seeding tenant '$TENANT' ($tenant_id)..."

  # --- Users (4 demo users for the tenant) -------------------------------
  # `users` is global (no tenant_id); membership lives in `tenant_users`.
  # `tenant_users.role` accepts viewer | publisher | curator | admin.
  log "  users + tenant_users..."
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
WITH demo(email, display_name, role) AS (
  VALUES
    ('alice@acme.test', 'Alice Admin',     'admin'),
    ('bob@acme.test',   'Bob Curator',     'curator'),
    ('carol@acme.test', 'Carol Publisher', 'publisher'),
    ('dave@acme.test',  'Dave Viewer',     'viewer')
),
upsert_users AS (
  INSERT INTO users (email, display_name)
  SELECT email, display_name FROM demo
  ON CONFLICT (email) DO UPDATE SET display_name = EXCLUDED.display_name
  RETURNING id, email
)
INSERT INTO tenant_users (tenant_id, user_id, role)
SELECT :'tenant_id'::uuid, u.id, d.role
  FROM upsert_users u
  JOIN demo d ON d.email = u.email
ON CONFLICT (tenant_id, user_id) DO UPDATE SET role = EXCLUDED.role;
SQL

  # --- Theme -------------------------------------------------------------
  # tenant_theme is keyed by tenant_id (PK). Schema uses `primary_` (no
  # `secondary_color`), with `accent` plus bg/fg/muted/border. We set a
  # plausible dev palette plus a placeholder logo URI.
  log "  tenant_theme..."
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
INSERT INTO tenant_theme (
    tenant_id, brand_name,
    primary_, primary_fg, accent,
    bg, fg, muted, muted_fg, border,
    radius, logo_uri, footer_branding
) VALUES (
    :'tenant_id'::uuid, 'Acme Corp',
    '#4f46e5', '#ffffff', '#06b6d4',
    '#ffffff', '#0f172a', '#f1f5f9', '#475569', '#e2e8f0',
    '0.5rem',
    'https://placehold.co/200x60/4f46e5/white/svg?text=Acme',
    true
)
ON CONFLICT (tenant_id) DO UPDATE SET
    brand_name      = EXCLUDED.brand_name,
    primary_        = EXCLUDED.primary_,
    primary_fg      = EXCLUDED.primary_fg,
    accent          = EXCLUDED.accent,
    bg              = EXCLUDED.bg,
    fg              = EXCLUDED.fg,
    muted           = EXCLUDED.muted,
    muted_fg        = EXCLUDED.muted_fg,
    border          = EXCLUDED.border,
    radius          = EXCLUDED.radius,
    logo_uri        = EXCLUDED.logo_uri,
    footer_branding = EXCLUDED.footer_branding;
SQL

  # --- OIDC SSO ----------------------------------------------------------
  log "  tenant_sso (OIDC demo config)..."
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
INSERT INTO tenant_sso (tenant_id, issuer_url, client_id, client_secret, default_role)
VALUES (
    :'tenant_id'::uuid,
    'https://accounts.google.com',
    'demo-client-id',
    'demo-secret-redacted',
    'viewer'
)
ON CONFLICT (tenant_id) DO UPDATE SET
    issuer_url    = EXCLUDED.issuer_url,
    client_id     = EXCLUDED.client_id,
    client_secret = EXCLUDED.client_secret,
    default_role  = EXCLUDED.default_role;
SQL

  # --- Custom domain ------------------------------------------------------
  # hostname is globally unique; verification_token is required.
  log "  tenant_custom_domains..."
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
INSERT INTO tenant_custom_domains (tenant_id, hostname, status, verification_token)
VALUES (
    :'tenant_id'::uuid,
    'skills.acme.com',
    'pending',
    'acme-verify-XYZ123'
)
ON CONFLICT (hostname) DO NOTHING;
SQL

  # --- Skill drafts -------------------------------------------------------
  # skill_drafts requires bundle_uri + bundle_sha256 NOT NULL. The status
  # CHECK accepts only pending|published|discarded — there's no needs_work.
  # We use a fake-but-namespaced bundle URI for these demo drafts (the
  # bundle never has to exist for the curator inbox to render).
  #
  # The third draft references a real published agent (code-reviewer) via
  # merge_proposal_skill_id; we look the id up by slug+kind. If absent we
  # leave the FK NULL.
  log "  skill_drafts..."
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
WITH proposal AS (
  SELECT id FROM skills
   WHERE tenant_id = :'tenant_id'::uuid
     AND slug = 'code-reviewer'
     AND kind = 'agent'
   LIMIT 1
),
demo_drafts(slug, description, when_to_use, tags, origin, notes, status,
            merge_id, merge_sim) AS (
  VALUES
    ('secret-scanner-helper',
     'Scan diffs for high-entropy secrets and known token patterns before they hit a remote.',
     'Run before every commit that touches config or env files',
     ARRAY['security','secrets','pre-commit'],
     'capture-scorer',
     'Auto-captured from a Stop-hook session where the user scrubbed an AWS key from staged files.',
     'pending',
     NULL::uuid,
     NULL::real),
    ('rust-axum-middleware-pattern',
     'Tower middleware skeleton with tenant extraction, structured logging, and rate-limit headers.',
     'When the user adds a new axum route guarded by a tenant context',
     ARRAY['rust','axum','middleware'],
     'cli',
     'Captured by hand; reviewer to dedupe against existing code-reviewer agent.',
     'pending',
     (SELECT id FROM proposal),
     0.87::real),
    ('ci-cd-troubleshoot',
     'Debug flaky GitHub Actions runs: cache invalidation, matrix expansion, secret-scope checks.',
     'When a CI job fails intermittently with no code change',
     ARRAY['ci','github-actions','debugging'],
     'claude-hook',
     'Reviewer flagged for rewrite — repurpose existing ci-cd-pipeline-builder content.',
     'pending',
     NULL::uuid,
     NULL::real)
)
INSERT INTO skill_drafts (
    tenant_id, slug, description, when_to_use, tags,
    origin, notes, status,
    merge_proposal_skill_id, merge_proposal_similarity,
    bundle_uri, bundle_sha256
)
SELECT
    :'tenant_id'::uuid, slug, description, when_to_use, tags,
    origin, notes, status,
    merge_id, merge_sim,
    'demo-seed://' || slug || '.tar.gz',
    repeat('0', 64)
FROM demo_drafts
WHERE NOT EXISTS (
    SELECT 1 FROM skill_drafts d
     WHERE d.tenant_id = :'tenant_id'::uuid
       AND d.slug = demo_drafts.slug
       AND d.status = 'pending'
);
SQL

  # --- Stack mappings -----------------------------------------------------
  # Map `rust+axum` to the imported agents/skills. If the slug isn't in
  # the catalog yet (because import-skills hasn't run), the row still
  # inserts — tenant_stack_mappings has no FK to skills.
  log "  tenant_stack_mappings..."
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
INSERT INTO tenant_stack_mappings (tenant_id, stack_tag, skill_slug) VALUES
    (:'tenant_id'::uuid, 'rust+axum', 'code-reviewer'),
    (:'tenant_id'::uuid, 'rust+axum', 'api-design-reviewer'),
    (:'tenant_id'::uuid, 'rust+axum', 'axum-handler'),
    (:'tenant_id'::uuid, 'security',  'secret-scanner-helper'),
    (:'tenant_id'::uuid, 'ci',        'ci-cd-pipeline-builder')
ON CONFLICT (tenant_id, stack_tag, skill_slug) DO NOTHING;
SQL

  # --- Usage events -------------------------------------------------------
  # Spread 30 events across the last 30 days, referencing up to 6 of the
  # tenant's published skills. event_kind ∈ download|view.
  # Idempotent: we wipe any prior demo-seed events first by deleting rows
  # tagged with the sentinel skill_id sequence (we don't have a marker
  # column so we instead delete only if total_event_count > 0 AND the
  # rows look synthetic — we keep it simple: only insert if zero rows
  # currently exist for this tenant).
  log "  skill_usage_events..."
  # psql `:'var'` substitution does NOT work inside `DO $$ ... $$` dollar
  # quotes, so we use a SET LOCAL on a custom GUC and read it via
  # current_setting() from inside the block.
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<SQL
BEGIN;
SET LOCAL skill_pool.tenant_id = '$tenant_id';
DO \$\$
DECLARE
    tid uuid := current_setting('skill_pool.tenant_id')::uuid;
    existing int;
    target_skills uuid[];
    s uuid;
    i int;
    kinds text[] := ARRAY['download','view'];
BEGIN
    SELECT count(*) INTO existing
      FROM skill_usage_events
     WHERE tenant_id = tid;

    IF existing >= 30 THEN
        RAISE NOTICE '  (skill_usage_events: already % rows, skipping demo seed)', existing;
        RETURN;
    END IF;

    SELECT array_agg(id) INTO target_skills FROM (
        SELECT id FROM skills
         WHERE tenant_id = tid
           AND status = 'published'
         ORDER BY created_at ASC
         LIMIT 6
    ) t;

    IF target_skills IS NULL OR array_length(target_skills, 1) IS NULL THEN
        RAISE NOTICE '  (skill_usage_events: no published skills yet, skipping)';
        RETURN;
    END IF;

    FOR i IN 1..30 LOOP
        s := target_skills[1 + (i % array_length(target_skills, 1))];
        INSERT INTO skill_usage_events (tenant_id, skill_id, event_kind, ts)
        VALUES (
            tid,
            s,
            kinds[1 + (i % 2)],
            now() - ((i % 30) || ' days')::interval - ((i * 53) % 1440 || ' minutes')::interval
        );
    END LOOP;
END\$\$;
COMMIT;
SQL

  log "done."
  log ""
  log "tenant state for '$TENANT':"
  psql_run -v ON_ERROR_STOP=1 -v tenant_id="$tenant_id" <<'SQL'
SELECT
    (SELECT count(*) FROM tenant_users   WHERE tenant_id = :'tenant_id'::uuid)            AS users,
    (SELECT count(*) FROM tenant_theme   WHERE tenant_id = :'tenant_id'::uuid)            AS themes,
    (SELECT count(*) FROM tenant_sso     WHERE tenant_id = :'tenant_id'::uuid)            AS sso,
    (SELECT count(*) FROM tenant_custom_domains WHERE tenant_id = :'tenant_id'::uuid)     AS domains,
    (SELECT count(*) FROM skill_drafts   WHERE tenant_id = :'tenant_id'::uuid)            AS drafts,
    (SELECT count(*) FROM tenant_stack_mappings WHERE tenant_id = :'tenant_id'::uuid)     AS mappings,
    (SELECT count(*) FROM skill_usage_events    WHERE tenant_id = :'tenant_id'::uuid)     AS usage_events;
SQL
}

main "$@"
