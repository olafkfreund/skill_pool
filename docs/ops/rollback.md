# Rollback playbook

For when a deploy goes wrong. Read top-to-bottom *before* you ever need
it; in the middle of an incident you skim section 4.

## 1. Operating model: forward-only

skill-pool migrations are **forward-only**. There are no `down.sql`
files, no auto-revert path, no in-place undo. Two reasons:

- **Down-migrations are usually wrong.** A migration that adds a NOT
  NULL column with a backfill cannot be "undone" without losing the
  backfill data. Down-migrations encourage operators to think rollback
  is a one-click affair; for non-trivial schema changes it isn't.
- **Restore is the real rollback.** A point-in-time recovery from a
  pre-deploy snapshot is the actual rollback, and it's tooling that
  every Postgres operator already runs nightly.

Two consequences worth internalising:

- **App rollback is cheap.** Redeploy the previous container image /
  release artifact. The new code reads the old schema fine because
  schema changes are always additive (add column, add table, add
  index) — never destructive in the same migration as a code change
  that depends on the new shape.
- **Schema rollback is expensive.** It is a database restore. Plan for
  the write traffic between snapshot-time and restore-time to be
  *lost*, not "rolled back".

## 2. The migration mechanism today

The server binary does **not** run migrations at startup. They are run
explicitly with `sqlx-cli`:

```bash
sqlx migrate run \
  --source server/migrations \
  --database-url "$SKILL_POOL_DATABASE_URL"
```

This decoupling is deliberate: a broken deploy of the app can never run
a migration as a side effect. The contract is:

| Step | Who | When |
|---|---|---|
| 1. Snapshot DB | operator / CI | immediately before each deploy |
| 2. `sqlx migrate run` | operator / CI | before starting the new app |
| 3. Start new app | systemd / k8s rollout | after migrations succeed |
| 4. Roll back app | operator / k8s | if smoke fails |
| 5. Restore DB | operator | only if step 2 left a broken schema |

`/app/migrations` is baked into the Docker image (`server/Dockerfile`
line 46) so the same migrations the binary expects are always one
directory away on the running host.

## 3. Pre-deploy checklist

Every deploy starts here. Skipping any of these makes rollback harder.

1. **Take a fresh snapshot.** Tag it with the commit SHA you're about
   to deploy:

   ```bash
   # Single-node Postgres
   pg_dump --format=custom --compress=9 \
     --dbname="$SKILL_POOL_DATABASE_URL" \
     --file="/var/backups/skill-pool/pre-$(git rev-parse --short HEAD).dump"

   # Managed (RDS)
   aws rds create-db-snapshot \
     --db-instance-identifier skill-pool-prod \
     --db-snapshot-identifier "skill-pool-pre-$(git rev-parse --short HEAD)"
   ```

2. **Stage bundles**, not just the DB. If the new code expects a
   bundle layout change (rare — bundles are immutable once published —
   but possible in admin tooling), snapshot the storage backend too.
   For `fs://`:

   ```bash
   tar -czf /var/backups/skill-pool/bundles-$(date -I).tar.gz \
     /var/lib/skill-pool/bundles
   ```

   For S3, ensure bucket versioning is on (you do this once when the
   bucket is created; verify with `aws s3api get-bucket-versioning`).

3. **Run migrations in a dry mode against a clone of prod.** If you
   have staging that mirrors prod schema:

   ```bash
   sqlx migrate info --source server/migrations \
     --database-url "$STAGING_DATABASE_URL"
   ```

   `migrate info` prints applied vs pending. Verify only the new
   migrations are pending; nothing already-applied has been edited
   (editing an already-applied migration is one of the few ways to
   wedge `_sqlx_migrations`).

4. **Verify migration filenames and content match Git.** sqlx stores
   the checksum of each migration in `_sqlx_migrations`. A file that
   has been edited after being applied to prod will refuse to run with
   `VersionMismatch`.

   ```sql
   SELECT version, description, success, checksum
     FROM _sqlx_migrations
     ORDER BY version DESC LIMIT 5;
   ```

5. **Have the rollback command ready.** Write the exact `pg_restore`
   or `aws rds restore-db-instance-from-db-snapshot` command into the
   deploy ticket *before* you press Go. Don't compose it under
   pressure.

## 4. Failure modes and what to do

Four scenarios cover ~95% of post-deploy "things broke" incidents.
Each has a different rollback shape.

### 4.1 App boots and serves, but behaves wrong (no DB harm)

Examples: a route returns the wrong shape; the catalog filters
incorrectly; embedding dedup is too aggressive.

This is the cheap case. The schema is fine; only the binary is wrong.

```bash
# Single-node (systemd):
sudo systemctl stop skill-pool-server
sudo install -m 0755 /var/backups/skill-pool/skill-pool-server.prev \
  /usr/local/bin/skill-pool-server
sudo systemctl start skill-pool-server

# Kubernetes:
kubectl rollout undo deployment/skill-pool-server -n skill-pool
```

No DB restore. The new migration(s) stay applied; the old binary
reads them fine because schema changes are additive. Verify with
`/v1/healthz`.

### 4.2 App fails to start against the new schema

Examples: migration ran, app boots, app crashes immediately because a
query references a column the binary doesn't know about, or
`_sqlx_migrations` got into a weird state.

Don't restore the DB. Restore the app:

```bash
# Step 1: stop the failing service
sudo systemctl stop skill-pool-server   # or kubectl scale --replicas=0

# Step 2: roll the binary back (same as 4.1)

# Step 3: leave the schema alone. The new migration is benign — the
# old binary doesn't use the new columns. Bring the old binary back up.
sudo systemctl start skill-pool-server
```

If `_sqlx_migrations` itself is in a weird state (you'll see
`VersionMissing` or `Dirty` errors in the boot log), that needs a
separate `sqlx migrate revert` *or* a careful hand fix — never on
prod-live, always against a restored snapshot first.

### 4.3 Migration corrupts data (rare but disastrous)

Examples: a backfill computed wrong values; a `UPDATE ... SET col =
DEFAULT` clobbered live data; a unique constraint added against an
existing duplicate locked the row.

This is the case the snapshot exists for. Restore.

```bash
# Single-node, recreate the database from the dump taken in §3.1:
sudo systemctl stop skill-pool-server
sudo -u postgres psql -c 'DROP DATABASE skillpool;'
sudo -u postgres psql -c 'CREATE DATABASE skillpool OWNER skillpool;'
sudo -u postgres psql -d skillpool -c 'CREATE EXTENSION vector;'
pg_restore --dbname="$SKILL_POOL_DATABASE_URL" --jobs=4 --no-owner \
  /var/backups/skill-pool/pre-<sha>.dump
# Bring the *old* binary back up against the restored schema:
sudo systemctl start skill-pool-server
```

```bash
# RDS (or Cloud SQL equivalent):
aws rds restore-db-instance-from-db-snapshot \
  --db-instance-identifier skill-pool-prod-restored \
  --db-snapshot-identifier skill-pool-pre-<sha>
# Then flip the application DSN to the restored instance and re-run
# the kubectl rollout undo.
```

Two things to accept:

- **Writes between snapshot-time and restore-time are lost.** Anything
  published, any audit-event recorded, any usage-event written after
  the snapshot is gone. Communicate this to users.
- **Bundle storage is on a separate failure domain.** A DB restore
  does not touch bundle bytes — those are still on disk / in S3.
  Re-running `pg_restore` against a populated bundle store works
  because `bundle_uri` in the restored `skills` row still points at
  the same object key (`{tenant_id}/{slug}/{version}.tar.gz`).
  Checksums baked into the bundle row let you validate this if you
  suspect drift.

### 4.4 Bundle storage gone but DB survives

Examples: the S3 bucket was emptied; the `fs://` volume was wiped.

The catalog DB knows every `bundle_uri` and (where present) the
SHA-256 of the bundle bytes. Bundles are immutable, so re-publishing
the same `(tenant_id, slug, version)` is safe — the upload path
overwrites the missing object and the row's existing checksum can be
re-verified.

```bash
# Inventory what needs re-publishing:
psql "$SKILL_POOL_DATABASE_URL" -c "
  SELECT tenant_id, slug, version, bundle_uri, sha256
    FROM skills
    WHERE status = 'published'
    ORDER BY created_at DESC
    LIMIT 20;
"
```

The recovery path is to walk the team Git mirror (or your CI artifact
store) where the original `SKILL.md` bundles live, and re-run
`skill-pool publish` for each. Use the `--version` flag to pin the
republish to the same version string the row already records — that
prevents a phantom version bump from breaking installs.

### 4.5 Both DB and bundles gone (catastrophic)

Re-create from your team's Git source-of-truth catalog. You lose:

- Tenants, tokens, theme rows — re-run `admin tenant-create`,
  `admin token-create`, theme edits.
- Audit log + SIEM exports — gone unless your SIEM had a copy.
- Use counts + last-used timestamps — reset to zero / NULL.

You keep: every `SKILL.md` that's in the Git mirror. Re-publish them
via `skill-pool publish` and the catalog re-bootstraps. Use the
[bootstrap-from-Git script](../../scripts/) if one exists; if not,
this is the time to write one.

## 5. What never to do

- **Never `DROP COLUMN` or `DROP TABLE` in a hotfix migration.**
  Always two-phase: stop reading the column (deploy that code first,
  leave the column in place), confirm it's truly unused for a release
  cycle, then drop in a later migration. The same applies to
  renaming.
- **Never edit `_sqlx_migrations` by hand on prod.** sqlx hashes each
  migration's content; tampering produces `VersionMismatch` errors
  that look like a separate incident. If you need to surgically fix
  the migrations table, do it on a restored snapshot first to
  validate.
- **Never run two deploys without snapshots between them.** Bisecting
  "which of the last two deploys broke it" without intermediate
  snapshots forces a worst-case restore. The pre-deploy snapshot in
  §3.1 takes seconds; the lost engineering hours from skipping one
  are paid for in a single bad week.
- **Never trust `pg_dump` you haven't restored.** Run a monthly drill
  that restores last night's dump into a scratch database. A backup
  that has never been restored is just a file.

## 6. Post-mortem checklist

After the dust settles:

1. Capture the migration filename(s) involved.
2. Capture the pre/post `_sqlx_migrations` state from the restored
   instance, and from the live instance.
3. Capture the wall-clock cost: snapshot age (how much data was
   lost), time to detect, time to roll back.
4. Open an issue tagged `incident` with the four data points above
   plus the corrective action (a code change, a runbook update, a
   guard rail in CI).
5. Add the failure mode to §4 if it wasn't already covered.

## 7. References

- `server/migrations/` — every migration that has ever shipped.
- `server/Dockerfile` line 46 — proves migrations ride along in the
  image.
- `docs/deploy/single-node.md` §1, `docs/deploy/nixos.md`,
  `docs/deploy/kubernetes.md` — environment-specific Postgres setup.
- `docs/ops/runbook.md` — incident response that complements this
  document (this one is "I'm doing a deploy"; the runbook is "I just
  got paged").
- `docs/ops/capacity.md` — sizing context for the restore target
  (the restored DB needs the same disk + RAM as the source).
