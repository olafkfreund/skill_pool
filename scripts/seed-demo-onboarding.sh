#!/usr/bin/env bash
#
# Idempotent seeder for the onboarding showcase demo.
#
# Verifies the live portal has the fixture state both halves of the demo
# expect:
#   - tenant `acme` exists
#   - admin token at ~/.config/skill-pool/config.toml (minted by scripts/seed-demo.sh)
#   - project `acme-billing-service` exists with curated items
#   - an active plan exists on that project (imports a fresh one if not)
#
# Run from anywhere. No-op if state is already correct.
#
# Environment overrides:
#   SKILL_POOL_API_BASE  (default: http://127.0.0.1:8080)
#   SKILL_POOL_TENANT    (default: acme)
#   SKILL_POOL_CONFIG    (default: ~/.config/skill-pool/config.toml)

set -euo pipefail

API="${SKILL_POOL_API_BASE:-http://127.0.0.1:8080}"
TENANT="${SKILL_POOL_TENANT:-acme}"
CONFIG="${SKILL_POOL_CONFIG:-$HOME/.config/skill-pool/config.toml}"
PROJECT="acme-billing-service"

die() { echo "seed-demo-onboarding: $*" >&2 ; exit 1 ; }
note() { echo "seed-demo-onboarding: $*" ; }

[ -f "$CONFIG" ] || die "$CONFIG missing — run scripts/seed-demo.sh first"
TOKEN=$(awk -F'"' '/^[[:space:]]*token[[:space:]]*=/ {print $2}' "$CONFIG")
[ -n "$TOKEN" ] || die "no token found in $CONFIG"

# 1. Portal up?
HEALTH=$(curl -fsS "$API/v1/healthz") || die "portal not reachable at $API"
echo "portal: $HEALTH"

# 2. Tenant `acme` reachable with the token?
TENANT_RESP=$(curl -fsS \
  -H "Host: $TENANT.localhost" \
  -H "Authorization: Bearer $TOKEN" \
  "$API/v1/tenant/projects" 2>&1) \
  || die "cannot list projects — token expired or wrong tenant?"

# 3. Project exists?
echo "$TENANT_RESP" | grep -q "\"slug\":\"$PROJECT\"" \
  || die "project '$PROJECT' missing — run scripts/seed-demo.sh first"
note "project '$PROJECT' present"

# 4. Active plan exists? If not, import a fresh one.
PLAN_STATUS=$(curl -s -o /dev/null -w "%{http_code}" \
  -H "Host: $TENANT.localhost" \
  -H "Authorization: Bearer $TOKEN" \
  "$API/v1/tenant/projects/$PROJECT/plan")

if [ "$PLAN_STATUS" = "200" ]; then
  note "active plan already present"
else
  note "no active plan — importing the demo fixture"
  PLAN_BODY=$(cat <<'EOF'
# Acme Billing Service — Q2 sprint plan

## North star
Ship the prorated-refunds path before the marketing site relaunch.

## In-flight
- Subscription upgrade flow (Stripe webhook → dunning state machine)
- Proration math: existing tests in `tests/billing_proration.rs` cover the four
  trial-to-paid transitions; new edge case (annual → monthly downgrade mid-cycle)
  needs coverage.
- DB migration `0042_subscription_grace_period.sql` — review needed before merge.

## Conventions
- Errors via `anyhow::Result` at the route layer; `thiserror::Error` enums in
  domain modules.
- Every webhook handler logs `event_id` + `subscription_id` for replay.
- Migrations are forward-only; no down scripts on this service.

## Active hooks
- `pre-commit`: `cargo fmt`, `sqlx prepare --check`, secret scan.
- `post-merge`: refresh local dev DB via `scripts/dev-reset.sh`.
EOF
)
  PAYLOAD=$(jq -nc --arg md "$PLAN_BODY" \
    '{source_type: "file", body_md: $md}')
  HTTP=$(curl -s -o /tmp/plan-import.json -w "%{http_code}" \
    -X POST \
    -H "Host: $TENANT.localhost" \
    -H "Authorization: Bearer $TOKEN" \
    -H "Content-Type: application/json" \
    --data-binary "$PAYLOAD" \
    "$API/v1/tenant/projects/$PROJECT/plan")
  if [ "$HTTP" != "200" ] && [ "$HTTP" != "201" ]; then
    cat /tmp/plan-import.json >&2
    die "plan import returned HTTP $HTTP"
  fi
  note "plan imported (HTTP $HTTP)"
fi

note "ready — both halves of the onboarding demo can record"
