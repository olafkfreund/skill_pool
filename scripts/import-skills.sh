#!/usr/bin/env bash
# import-skills.sh — populate the local skill-pool dev portal with content
# from borghei/Claude-Skills.
#
# Idempotent: a slug already present in `skills` for the target tenant
# (with the matching `kind`) is skipped. Re-running prints "skip <slug>"
# for every entry and produces no duplicate rows.
#
# Flow:
#   1. Verify the local API is up.
#   2. Clone (or reuse) /tmp/claude-skills-source.
#   3. Ensure ~/.config/skill-pool/config.toml points at the local server
#      with a tenant:admin token. Mint via `skill-pool-server admin
#      token-create` if missing.
#   4. For each engineering + data-analytics SKILL.md → publish kind=skill.
#   5. For each agents/{engineering,product,c-level}/*.md → wrap into a
#      throwaway dir with SKILL.md and publish kind=agent.
#
# License note: borghei/Claude-Skills is MIT + Commons Clause. The Commons
# Clause restricts selling the software itself but not redistribution as
# bundled content; the SKILL.md `license:` frontmatter is preserved by the
# publish path.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

API_URL="${SKILL_POOL_API_URL:-http://127.0.0.1:8080}"
TENANT="${SKILL_POOL_TENANT:-acme}"
SOURCE_DIR="${CLAUDE_SKILLS_SOURCE:-/tmp/claude-skills-source}"
SOURCE_REPO="${CLAUDE_SKILLS_REPO:-borghei/Claude-Skills}"
DB_URL="${SKILL_POOL_DATABASE_URL:-postgres://skillpool:skillpool@127.0.0.1:55432/skillpool}"
PG_ARGS=(-h 127.0.0.1 -p "${PGPORT:-55432}" -U skillpool -d skillpool)
export PGPASSWORD="${PGPASSWORD:-skillpool}"

CLI="$REPO_ROOT/target/debug/skill-pool"
SERVER_BIN="$REPO_ROOT/target/debug/skill-pool-server"
CONFIG_FILE="${SKILL_POOL_CONFIG:-$HOME/.config/skill-pool/config.toml}"

log() { printf '%s\n' "$*"; }
err() { printf 'error: %s\n' "$*" >&2; }

ensure_binaries() {
  if [[ ! -x "$CLI" ]]; then
    log "building skill-pool CLI..."
    cargo build -p skill-pool-cli --bin skill-pool >&2
  fi
  if [[ ! -x "$SERVER_BIN" ]]; then
    log "building skill-pool-server..."
    cargo build -p skill-pool-server --bin skill-pool-server >&2
  fi
}

ensure_api_up() {
  if ! curl -fsS "$API_URL/v1/healthz" >/dev/null 2>&1; then
    err "API at $API_URL is not reachable — start the dev server first"
    exit 1
  fi
}

ensure_repo() {
  if [[ -d "$SOURCE_DIR/.git" ]]; then
    log "reusing existing clone at $SOURCE_DIR"
    return
  fi
  log "cloning $SOURCE_REPO into $SOURCE_DIR..."
  gh repo clone "$SOURCE_REPO" "$SOURCE_DIR" >&2
}

ensure_login() {
  if [[ -f "$CONFIG_FILE" ]] && grep -q "token = \"spk_" "$CONFIG_FILE" 2>/dev/null; then
    local cfg_url
    cfg_url="$(grep -E '^url = ' "$CONFIG_FILE" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
    if [[ "$cfg_url" == "$API_URL" ]]; then
      log "using existing CLI config at $CONFIG_FILE"
      return
    fi
    log "config exists but points at $cfg_url; refreshing for $API_URL"
  fi

  log "minting tenant:admin token for tenant=$TENANT..."
  local token
  token="$(
    SKILL_POOL_DATABASE_URL="$DB_URL" "$SERVER_BIN" admin token-create \
      --tenant "$TENANT" \
      --name "demo-import-$(date +%s)" \
      --scope "tenant:admin skills:read skills:publish" \
    | awk '/^  spk_/ { print $1; exit }'
  )"
  if [[ -z "$token" ]]; then
    err "token-create did not produce a raw token"
    exit 1
  fi

  log "saving config via skill-pool login..."
  printf '%s\n' "$token" | "$CLI" login --registry "$API_URL" --tenant "$TENANT" >&2
}

# Look up the tenant UUID once so we can check existence cheaply per-slug.
TENANT_ID=""
resolve_tenant_id() {
  TENANT_ID="$(psql "${PG_ARGS[@]}" -tA -c "select id from tenants where slug='$TENANT'")"
  if [[ -z "$TENANT_ID" ]]; then
    err "tenant '$TENANT' not found in database"
    exit 1
  fi
}

# Returns 0 if a (slug, kind) pair already exists for our tenant.
exists_in_catalog() {
  local slug="$1" kind="$2"
  local count
  count="$(psql "${PG_ARGS[@]}" -tA -c \
    "select count(*) from skills where tenant_id='$TENANT_ID' and slug='$slug' and kind='$kind'")"
  [[ "$count" != "0" ]]
}

# Pull `metadata.version: X.Y.Z` from a SKILL.md frontmatter block, defaulting
# to 1.0.0. Uses awk so we don't depend on yq.
extract_version() {
  local file="$1"
  awk '
    /^---[[:space:]]*$/ { fm = !fm; next }
    fm && /^[[:space:]]*version:[[:space:]]/ {
      sub(/^[[:space:]]*version:[[:space:]]*/, "")
      gsub(/["\047]/, "")
      print
      exit
    }
  ' "$file" | head -1 | tr -d '[:space:]' || true
}

SKILL_PUBLISHED=0
SKILL_SKIPPED=0
AGENT_PUBLISHED=0
AGENT_SKIPPED=0

publish_skill_dir() {
  local skill_dir="$1"
  local slug
  slug="$(basename "$skill_dir")"

  if exists_in_catalog "$slug" "skill"; then
    log "  skip $slug (skill, already in catalog)"
    SKILL_SKIPPED=$((SKILL_SKIPPED + 1))
    return
  fi

  local version
  version="$(extract_version "$skill_dir/SKILL.md")"
  [[ -z "$version" ]] && version="1.0.0"

  if "$CLI" publish "$skill_dir" --version "$version" --kind skill >/dev/null 2>&1; then
    log "  publish $slug@$version (skill)"
    SKILL_PUBLISHED=$((SKILL_PUBLISHED + 1))
  else
    err "  failed to publish skill $slug"
    "$CLI" publish "$skill_dir" --version "$version" --kind skill 2>&1 | sed 's/^/    /' >&2 || true
  fi
}

publish_agent_file() {
  local agent_file="$1"
  local slug
  slug="$(basename "$agent_file" .md)"

  if exists_in_catalog "$slug" "agent"; then
    log "  skip $slug (agent, already in catalog)"
    AGENT_SKIPPED=$((AGENT_SKIPPED + 1))
    return
  fi

  local version
  version="$(extract_version "$agent_file")"
  [[ -z "$version" ]] && version="1.0.0"

  local tmpdir
  tmpdir="$(mktemp -d)"
  mkdir -p "$tmpdir/$slug"
  cp "$agent_file" "$tmpdir/$slug/SKILL.md"

  if "$CLI" publish "$tmpdir/$slug" --version "$version" --kind agent >/dev/null 2>&1; then
    log "  publish $slug@$version (agent)"
    AGENT_PUBLISHED=$((AGENT_PUBLISHED + 1))
  else
    err "  failed to publish agent $slug"
    "$CLI" publish "$tmpdir/$slug" --version "$version" --kind agent 2>&1 | sed 's/^/    /' >&2 || true
  fi

  rm -rf "$tmpdir"
}

import_skill_category() {
  local category_dir="$1"
  if [[ ! -d "$category_dir" ]]; then
    return
  fi
  local found=0
  while IFS= read -r -d '' skill_md; do
    found=1
    publish_skill_dir "$(dirname "$skill_md")"
  done < <(find "$category_dir" -mindepth 2 -maxdepth 2 -name SKILL.md -print0 | sort -z)
  if [[ "$found" == "0" ]]; then
    log "  (no SKILL.md files under $category_dir — skipping)"
  fi
}

import_agent_category() {
  local agent_dir="$1"
  if [[ ! -d "$agent_dir" ]]; then
    log "  (no agents under $agent_dir — skipping)"
    return
  fi
  while IFS= read -r -d '' agent_file; do
    publish_agent_file "$agent_file"
  done < <(find "$agent_dir" -mindepth 1 -maxdepth 1 -name '*.md' ! -name 'CLAUDE.md' -print0 | sort -z)
}

main() {
  ensure_binaries
  ensure_api_up
  ensure_repo
  ensure_login
  resolve_tenant_id

  log "importing skills (category: engineering)..."
  import_skill_category "$SOURCE_DIR/engineering"
  for extra in cli data-analytics documentation standards; do
    log "importing skills (category: $extra)..."
    import_skill_category "$SOURCE_DIR/$extra"
  done

  for agent_cat in engineering product c-level; do
    log "importing agents (category: $agent_cat)..."
    import_agent_category "$SOURCE_DIR/agents/$agent_cat"
  done

  log ""
  log "summary:"
  log "  skills:  $SKILL_PUBLISHED published, $SKILL_SKIPPED skipped"
  log "  agents:  $AGENT_PUBLISHED published, $AGENT_SKIPPED skipped"
  log "✓ imported $SKILL_PUBLISHED skills + $AGENT_PUBLISHED agents"
}

main "$@"
