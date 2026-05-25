#!/usr/bin/env bash
#
# Re-records the two-half onboarding showcase end-to-end.
#
# Produces (overwriting in place):
#   docs/demo/onboarding-cli.webm
#   docs/demo/onboarding-cli.gif
#   docs/demo/onboarding-portal.webm
#   docs/demo/onboarding-portal.gif
#
# Assumes:
#   * portal up at $PORTAL_BASE_URL (default http://127.0.0.1:3030)
#   * server up at $SKILL_POOL_API_BASE (default http://127.0.0.1:8080)
#   * admin token in ~/.config/skill-pool/config.toml
#   * `vhs` and `ffmpeg` on PATH (devshell or system)
#
# Dependencies the script installs on first run:
#   * scripts/playwright/node_modules (npm install)
#   * playwright's chromium binary (npx playwright install chromium)
#
# Usage:
#   ./scripts/record-onboarding-demo.sh

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PORTAL="${PORTAL_BASE_URL:-http://127.0.0.1:3030}"
OUT="docs/demo"

note() { printf '\n--- %s ---\n' "$*"; }

note "1. Seed fixture state"
bash scripts/seed-demo-onboarding.sh

note "2. Record CLI half (vhs)"
mkdir -p "$OUT"
# vhs is an interactive renderer; insecure-pkg gate hits via nix devshell so we
# allow it impurely. If vhs is already on PATH, use it directly.
if command -v vhs >/dev/null 2>&1; then
  vhs scripts/demo-onboarding-cli.tape
else
  NIXPKGS_ALLOW_INSECURE=1 nix develop --impure -c vhs scripts/demo-onboarding-cli.tape
fi
# vhs writes outputs to the paths declared inside the tape (docs/demo/...).

note "3. Record portal half (playwright)"
pushd scripts/playwright >/dev/null
if [ ! -d node_modules ] || [ ! -x node_modules/.bin/playwright ]; then
  # NODE_ENV=production is sticky on many dev machines and would skip
  # devDeps. Force inclusion explicitly.
  NODE_ENV=development npm install --no-fund --no-audit --include=dev
fi
# We use the system-installed google-chrome (channel: 'chrome' in the
# playwright config) rather than playwright's bundled chromium. The
# downloaded chromium needs FHS-style /lib paths and won't launch on
# NixOS without a buildFHSUserEnv wrapper.
if ! command -v google-chrome >/dev/null 2>&1; then
  echo "FATAL: 'google-chrome' not on PATH — install it via your system pkg manager (NixOS: programs.chromium.enable or environment.systemPackages = [ pkgs.google-chrome ])" >&2
  exit 1
fi
SP_DEMO_CHROME="${SP_DEMO_CHROME:-$(command -v google-chrome)}" \
PORTAL_BASE_URL="$PORTAL" \
  ./node_modules/.bin/playwright test --reporter=line
popd >/dev/null

# Move the video out of test-results/ into docs/demo/.
VIDEO=$(find scripts/playwright/test-results -name "*.webm" -type f | head -1)
if [ -z "$VIDEO" ]; then
  echo "FATAL: no webm produced by playwright; check scripts/playwright/test-results" >&2
  exit 1
fi
cp "$VIDEO" "$OUT/onboarding-portal.webm"

note "4. Render gifs from both webms (ffmpeg)"
for stem in onboarding-cli onboarding-portal; do
  webm="$OUT/$stem.webm"
  gif="$OUT/$stem.gif"
  [ -f "$webm" ] || { echo "missing $webm" >&2 ; exit 1; }
  # VHS already produces the cli gif. Skip ffmpeg for it.
  if [ "$stem" = "onboarding-cli" ] && [ -f "$gif" ]; then
    continue
  fi
  ffmpeg -y -i "$webm" \
    -vf "fps=10,scale=800:-1:flags=lanczos,split[s0][s1];[s0]palettegen=stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=5:diff_mode=rectangle" \
    -loop 0 "$gif" 2>&1 | tail -3
done

note "5. Done — output files"
ls -lh "$OUT"
