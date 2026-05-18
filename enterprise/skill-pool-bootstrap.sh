#!/usr/bin/env bash
# skill-pool bootstrap helper.
#
# Reads the per-machine skill-pool token from SKILL_POOL_TOKEN_FILE
# (defaults to /etc/skill-pool/token, MDM-deployed) and prints it on
# stdout. This is the contract `skill-pool` CLI uses when invoked without
# a `--token` flag and without ~/.skill-pool/config.toml configured.
#
# Designed for fleet deployments where:
#   - Anthropic's API key is managed by Anthropic's own apiKeyHelper
#   - The skill-pool registry token is rotated centrally and pushed via MDM
#   - Neither secret should live in user-editable config files

set -euo pipefail

TOKEN_FILE="${SKILL_POOL_TOKEN_FILE:-/etc/skill-pool/token}"

if [ ! -r "$TOKEN_FILE" ]; then
  echo "error: $TOKEN_FILE not readable" >&2
  echo "       deploy it via MDM, or set SKILL_POOL_TOKEN_FILE to point elsewhere." >&2
  exit 1
fi

# Strip whitespace; some MDM tooling adds trailing newlines.
tr -d '[:space:]' <"$TOKEN_FILE"
