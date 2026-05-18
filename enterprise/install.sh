#!/usr/bin/env bash
# skill-pool Claude Code managed-settings installer.
#
# Downloads a tenant-tailored managed-settings.json from the registry and
# installs it to the right OS path. Idempotent. Designed for fleet rollout
# via Ansible / Jamf / Intune scripts, plus one-off SSH invocations.
#
# USAGE:
#     install.sh --registry https://acme.skill-pool.example.com \
#                --tenant acme \
#                --admin-token spk_xxx \
#                [--target /override/managed-settings.json] \
#                [--token-out /etc/skill-pool/token]
#
# What it does:
#   1. GET /v1/enterprise/managed-settings from the registry with the admin
#      token; saves the body to the OS-appropriate managed-settings path.
#   2. Optionally writes a per-machine bootstrap token to --token-out so
#      `SKILL_POOL_TOKEN_FILE=/etc/skill-pool/token` (pinned by the
#      managed-settings env) resolves at runtime.
#   3. Verifies the result by parsing the JSON with python3.

set -euo pipefail

LOG() { printf '\n=== %s ===\n' "$*"; }
DIE() {
  echo "error: $*" >&2
  exit 1
}

REGISTRY=""
TENANT=""
ADMIN_TOKEN=""
TARGET=""
TOKEN_OUT=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --registry)
      REGISTRY="$2"
      shift 2
      ;;
    --tenant)
      TENANT="$2"
      shift 2
      ;;
    --admin-token)
      ADMIN_TOKEN="$2"
      shift 2
      ;;
    --target)
      TARGET="$2"
      shift 2
      ;;
    --token-out)
      TOKEN_OUT="$2"
      shift 2
      ;;
    -h | --help)
      sed -n '2,30p' "$0"
      exit 0
      ;;
    *) DIE "unknown flag: $1" ;;
  esac
done

[ -n "$REGISTRY" ] || DIE "--registry is required"
[ -n "$TENANT" ] || DIE "--tenant is required"
[ -n "$ADMIN_TOKEN" ] || DIE "--admin-token is required (must have tenant:admin scope)"

# Resolve OS-specific managed-settings path if not overridden.
if [ -z "$TARGET" ]; then
  case "$(uname -s)" in
    Darwin) TARGET="/Library/Application Support/ClaudeCode/managed-settings.json" ;;
    Linux | CYGWIN* | MINGW* | MSYS*) TARGET="/etc/claude-code/managed-settings.json" ;;
    *) DIE "unsupported OS $(uname -s); pass --target explicitly" ;;
  esac
fi

LOG "downloading managed-settings.json"
TARGET_DIR="$(dirname "$TARGET")"
sudo mkdir -p "$TARGET_DIR"
TMP="$(mktemp)"
trap 'rm -f "$TMP"' EXIT

HTTP_CODE=$(curl -sS -w '%{http_code}' -o "$TMP" \
  -H "Authorization: Bearer $ADMIN_TOKEN" \
  -H "X-Skill-Pool-Tenant: $TENANT" \
  "$REGISTRY/v1/enterprise/managed-settings")
[ "$HTTP_CODE" = "200" ] || DIE "registry returned $HTTP_CODE: $(cat "$TMP")"

# Validate it parses.
python3 -c 'import json,sys; json.load(open(sys.argv[1]))' "$TMP" ||
  DIE "downloaded file is not valid JSON"

LOG "installing to $TARGET"
sudo install -m 0644 "$TMP" "$TARGET"

if [ -n "$TOKEN_OUT" ]; then
  LOG "writing bootstrap token to $TOKEN_OUT"
  TOKEN_DIR="$(dirname "$TOKEN_OUT")"
  sudo mkdir -p "$TOKEN_DIR"
  # Token file holds *only* the token. Permissions: 0600, owner root by
  # default; ops should chown to the unprivileged user running skill-pool.
  echo "$ADMIN_TOKEN" | sudo tee "$TOKEN_OUT" >/dev/null
  sudo chmod 0600 "$TOKEN_OUT"
fi

LOG "done"
echo "  Claude Code will pick up managed-settings.json on next launch."
echo "  Verify with:  cat \"$TARGET\""
