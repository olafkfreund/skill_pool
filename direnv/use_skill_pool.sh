# shellcheck shell=bash
# skill-pool direnv extension.
#
# Install:
#   skill-pool direnv-install            # copies this file into
#                                         #   ~/.config/direnv/lib/use_skill_pool.sh
#
# Then in each project's `.envrc`:
#   use skill_pool                       # silent ensure on shell entry
#   use skill_pool bootstrap             # first-time: detect + recommend + install
#
# Design goals:
#   - Silent on the happy path. Only chirps on changes/errors.
#   - Never blocks shell entry. Logs warnings, returns 0.
#   - Cheap. Calls `skill-pool ensure --quiet` which is fast when nothing
#     changed (no network on cache hit).

use_skill_pool() {
  local mode="${1:-ensure}"

  if ! command -v skill-pool >/dev/null 2>&1; then
    log_status "skill-pool: CLI not on PATH (skipping)"
    return 0
  fi

  if ! command -v claude >/dev/null 2>&1; then
    : # Claude Code not installed locally — that's fine; the symlinks still
    # land at .claude/skills/ and Claude on another machine will discover.
  fi

  case "$mode" in
    ensure | "")
      if [[ ! -f .skill-pool/manifest.toml ]]; then
        log_status "skill-pool: no .skill-pool/manifest.toml (run 'skill-pool init' or 'skill-pool bootstrap')"
        return 0
      fi
      # `ensure --quiet` exits 0 on success with no output. Any output here
      # means something interesting (or an error) happened.
      local output
      if ! output=$(skill-pool ensure --quiet 2>&1); then
        log_error "skill-pool ensure failed:"
        log_error "$output"
        return 0
      fi
      if [[ -n "$output" ]]; then
        log_status "$output"
      fi
      ;;
    bootstrap)
      skill-pool bootstrap --yes 2>&1 | while IFS= read -r line; do
        log_status "skill-pool: $line"
      done
      ;;
    *)
      log_error "skill-pool: unknown mode '$mode' (expected: ensure | bootstrap)"
      return 0
      ;;
  esac

  # Watch the manifest + the cookie that skill-pool ensure touches so that
  # `direnv reload` picks up changes without forcing a manual edit of the
  # .envrc.
  watch_file .skill-pool/manifest.toml
}
