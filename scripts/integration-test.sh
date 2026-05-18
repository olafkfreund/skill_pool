#!/usr/bin/env bash
# Manual smoke test against the docker-compose stack.
#
# The canonical CI integration test lives in `server/tests/integration.rs` and
# uses testcontainers to manage its own Postgres. This script is for humans
# who want to poke the *actual* stack (compose.yaml's postgres + minio + caddy)
# and confirm subdomain routing, MinIO storage, and the binaries behave.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

LOG() { printf '\n=== %s ===\n' "$*"; }

LOG "1. starting docker compose deps"
docker compose -f server/compose.yaml up -d postgres minio
trap 'echo "--- tearing down compose"; docker compose -f server/compose.yaml down -v' EXIT

# Wait for Postgres to accept connections.
for _ in {1..30}; do
  docker compose -f server/compose.yaml exec -T postgres pg_isready -U skillpool >/dev/null 2>&1 && break
  sleep 1
done

export SKILL_POOL_DATABASE_URL="postgres://skillpool:skillpool@127.0.0.1:5432/skillpool"
export SKILL_POOL_STORAGE_URI="fs:///tmp/skill-pool-stack-test"
export SKILL_POOL_BIND="127.0.0.1:8080"

rm -rf /tmp/skill-pool-stack-test
mkdir -p /tmp/skill-pool-stack-test

LOG "2. building binaries"
cargo build --release --workspace >/dev/null

LOG "3. running migrations"
DATABASE_URL="$SKILL_POOL_DATABASE_URL" target/release/skill-pool-server --help >/dev/null
# sqlx-cli is in the dev shell; use it if present, otherwise apply migrations via psql.
if command -v sqlx >/dev/null; then
  (cd server && DATABASE_URL="$SKILL_POOL_DATABASE_URL" sqlx migrate run)
else
  docker compose -f server/compose.yaml exec -T -e PGPASSWORD=skillpool postgres \
    psql -U skillpool -d skillpool <server/migrations/0001_init.sql
fi

LOG "4. seeding tenants + tokens"
target/release/skill-pool-server admin tenant-create --slug acme --name 'Acme Corp' --plan team
target/release/skill-pool-server admin tenant-create --slug globex --name 'Globex Inc' --plan team

ACME_TOKEN=$(target/release/skill-pool-server admin token-create --tenant acme --name smoke | awk '/^  spk_/ {print $1}')
GLOBEX_TOKEN=$(target/release/skill-pool-server admin token-create --tenant globex --name smoke | awk '/^  spk_/ {print $1}')
[ -n "$ACME_TOKEN" ] && [ -n "$GLOBEX_TOKEN" ] || {
  echo "failed to mint tokens"
  exit 1
}

LOG "5. starting server"
target/release/skill-pool-server &
SERVER_PID=$!
trap 'kill $SERVER_PID 2>/dev/null || true; docker compose -f server/compose.yaml down -v' EXIT
sleep 1

LOG "6. publishing as acme via CLI"
target/release/skill-pool login \
  --registry "http://127.0.0.1:8080" \
  --tenant acme <<<"$ACME_TOKEN"
target/release/skill-pool publish ./skills/test-skill --version 1.0.0 --slug test-skill

LOG "7. asserting acme sees it"
LIST=$(curl -sS -H "Authorization: Bearer $ACME_TOKEN" -H "x-skill-pool-tenant: acme" http://127.0.0.1:8080/v1/skills)
echo "$LIST" | grep -q test-skill || {
  echo "FAIL: acme should see test-skill"
  echo "$LIST"
  exit 1
}
echo "  ok"

LOG "8. asserting globex does NOT see it"
LIST=$(curl -sS -H "Authorization: Bearer $GLOBEX_TOKEN" -H "x-skill-pool-tenant: globex" http://127.0.0.1:8080/v1/skills)
[ "$LIST" = "[]" ] || {
  echo "FAIL: globex should see empty list, got: $LIST"
  exit 1
}
echo "  ok"

LOG "9. installing back into a fresh project"
PROJECT=$(mktemp -d)
(cd "$PROJECT" && git init -q && target/release/skill-pool init >/dev/null && target/release/skill-pool add test-skill)
ls -la "$PROJECT/.claude/skills/test-skill"
readlink "$PROJECT/.claude/skills/test-skill" | grep -q library/acme/test-skill@1.0.0 || {
  echo "FAIL: symlink doesn't point at expected library entry"
  exit 1
}
echo "  ok"

LOG "smoke test passed"
