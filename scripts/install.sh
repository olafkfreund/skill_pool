#!/usr/bin/env bash
# skill-pool Phase 0 installer.
#
# Symlinks one or more skills from a source library directory into a target
# Claude Code skills directory. Idempotent. No server. No network.
#
# Replaced in Phase 1 by the `skill-pool` Rust CLI (`skill-pool ensure`,
# `skill-pool add <slug>`).

set -euo pipefail

LIBRARY_DEFAULT="${SKILL_POOL_LIBRARY:-$HOME/.skill-pool/library}"
TARGET_DEFAULT="${SKILL_POOL_TARGET:-$HOME/.claude/skills}"

usage() {
  cat <<EOF
skill-pool installer (Phase 0)

USAGE:
    install.sh [--library DIR] [--target DIR] [--dry-run] [--list] <skill>...
    install.sh --uninstall [--target DIR] <skill>...

OPTIONS:
    --library DIR    Source directory containing <skill>/SKILL.md entries.
                     Default: \$SKILL_POOL_LIBRARY or ~/.skill-pool/library
    --target DIR     Destination skills directory.
                     Default: \$SKILL_POOL_TARGET or ~/.claude/skills
    --dry-run        Show what would happen; make no changes.
    --list           List skills available in the library; exit.
    --uninstall      Remove symlinks for the given skills instead of installing.
    -h, --help       Show this help.

EXAMPLES:
    # Install the bundled test skill from this repo into your user scope:
    install.sh --library ./skills test-skill

    # Install into a project rather than user scope:
    install.sh --library ./skills --target ./.claude/skills test-skill

    # See what's available:
    install.sh --library ./skills --list

EXIT CODES:
    0  success
    1  bad usage
    2  skill not found in library
    3  symlink conflict (target exists and is not our symlink)
EOF
}

log() { printf '%s\n' "$*" >&2; }
die() {
  log "error: $*"
  exit "${2:-1}"
}
info() { printf '  %s\n' "$*"; }

library="$LIBRARY_DEFAULT"
target="$TARGET_DEFAULT"
dry_run=0
list_only=0
uninstall=0
skills=()

while (($# > 0)); do
  case "$1" in
    --library)
      library="${2:?--library needs a value}"
      shift 2
      ;;
    --target)
      target="${2:?--target needs a value}"
      shift 2
      ;;
    --dry-run)
      dry_run=1
      shift
      ;;
    --list)
      list_only=1
      shift
      ;;
    --uninstall)
      uninstall=1
      shift
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    --)
      shift
      while (($# > 0)); do
        skills+=("$1")
        shift
      done
      ;;
    -*) die "unknown option: $1" ;;
    *)
      skills+=("$1")
      shift
      ;;
  esac
done

# Resolve to absolute paths so symlinks survive cwd changes.
library="$(cd "$library" 2>/dev/null && pwd)" || die "library not found: $library"

if ((list_only)); then
  printf 'Library: %s\n\n' "$library"
  found=0
  while IFS= read -r -d '' skill_md; do
    found=1
    slug="$(basename "$(dirname "$skill_md")")"
    # Extract description from frontmatter (best-effort; first matching line).
    desc="$(awk '
      /^---$/ { in_fm = !in_fm; next }
      in_fm && /^description:/ {
        sub(/^description: */, "")
        print
        exit
      }
    ' "$skill_md")"
    printf '  %-32s %s\n' "$slug" "${desc:-(no description)}"
  done < <(find "$library" -mindepth 2 -maxdepth 2 -name SKILL.md -print0 | sort -z)
  ((found)) || log "(no skills found in $library)"
  exit 0
fi

((${#skills[@]} > 0)) || {
  usage
  exit 1
}

mkdir -p "$target"
target="$(cd "$target" && pwd)"

for slug in "${skills[@]}"; do
  src="$library/$slug"
  dst="$target/$slug"

  if ((uninstall)); then
    if [[ -L "$dst" ]]; then
      info "unlink: $dst"
      ((dry_run)) || rm "$dst"
    elif [[ -e "$dst" ]]; then
      die "refusing to remove non-symlink: $dst" 3
    else
      info "absent (skip): $dst"
    fi
    continue
  fi

  [[ -d "$src" && -f "$src/SKILL.md" ]] || die "skill not in library: $slug (looked for $src/SKILL.md)" 2

  if [[ -L "$dst" ]]; then
    current="$(readlink -f -- "$dst")"
    if [[ "$current" == "$src" ]]; then
      info "ok    (already linked): $slug"
      continue
    fi
    info "relink: $slug ($dst -> $src, was $current)"
    ((dry_run)) || ln -snf "$src" "$dst"
  elif [[ -e "$dst" ]]; then
    die "refusing to overwrite non-symlink: $dst" 3
  else
    info "link:   $slug ($dst -> $src)"
    ((dry_run)) || ln -s "$src" "$dst"
  fi
done

if ((dry_run)); then
  log "(dry-run; no changes made)"
fi
