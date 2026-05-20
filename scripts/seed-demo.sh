#!/usr/bin/env bash
# seed-demo.sh — single entrypoint for the showcase demo data.
#
# 1. Imports the borghei/Claude-Skills catalog into the local registry
#    (engineering + data-analytics skills + agents/{engineering,product,c-level}).
# 2. Seeds tenant-level state for the `acme` tenant (users, theme, SSO,
#    custom domain, drafts, stack mappings, usage events).
#
# Both steps are idempotent — re-running this script is safe.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

./scripts/import-skills.sh
./scripts/seed-tenant.sh

echo "✓ portal seeded — open http://razer.lan:3000"
