# skill-pool — onboarding showcase (CLI half)
#
# Records the developer-side onboarding flow for the docs/demo/ landing
# page. Renders into docs/demo/onboarding-cli.{webm,gif} via:
#
#   ./scripts/record-onboarding-demo.sh
#
# Or standalone:
#
#   NIXPKGS_ALLOW_INSECURE=1 nix develop --impure -c vhs scripts/demo-onboarding-cli.tape
#
# Assumes scripts/seed-demo-onboarding.sh has run and the portal is up at
# http://127.0.0.1:8080 with tenant `acme`. The recorded token is the one
# the seeder mints under ~/.config/skill-pool/config.toml.

Output docs/demo/onboarding-cli.webm
Output docs/demo/onboarding-cli.gif

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 760
Set Theme "Catppuccin Mocha"
Set TypingSpeed 30ms
Set PlaybackSpeed 1
Set Padding 30

# --- Hidden setup: scratch dir, isolated config, fresh cargo project ---
Hide
Type "export PS1='$ '"
Enter
Type "export PROMPT_COMMAND="
Enter
Type "export _ZO_DOCTOR=0"
Enter
# Use the system skill-pool binary (kept in sync with the deployed portal).
Type "export PATH=/run/current-system/sw/bin:$PATH"
Enter
Type "clear"
Enter
Type "rm -rf /tmp/sp-onboard && mkdir -p /tmp/sp-onboard && cd /tmp/sp-onboard"
Enter
# Isolated XDG so `skill-pool login` doesn't touch the real ~/.config.
Type "export XDG_CONFIG_HOME=/tmp/sp-onboard/xdg"
Enter
# Stand up a fake Rust project so `init` / `bootstrap` light up.
Type "cargo init --name billing-service -q 2>/dev/null"
Enter
Type "git remote add origin https://github.com/acme/billing-service.git 2>/dev/null"
Enter
Type "clear"
Enter
Sleep 500ms
Show

# ─────────────────────────────────────────────────────────────────────
# Act 1 — A developer just cloned the repo. Onboard it.
# ─────────────────────────────────────────────────────────────────────
Type "# I just cloned acme/billing-service. Let's wire up its skill bundle."
Sleep 800ms
Enter
Sleep 400ms

Type "skill-pool init --project acme-billing-service"
Enter
Sleep 2500ms

Type "cat .skill-pool/manifest.toml"
Enter
Sleep 2800ms

Type "# Authenticate against the team's registry."
Sleep 700ms
Enter
Sleep 300ms

Type "skill-pool login --registry http://127.0.0.1:8080 --tenant acme"
Enter
Sleep 1300ms
# `skill-pool login` reads the token from stdin via rpassword.
Type "spk_2a01adeff80a62f3f87ea287d0103cc2eedd85d13fdd0eb7e6ac5fbbca68af0b"
Enter
Sleep 1800ms

Type "# Bootstrap: pull the curated bundle the team's curator pinned."
Sleep 700ms
Enter
Sleep 300ms

Type "skill-pool bootstrap --yes"
Enter
Sleep 7000ms

# ─────────────────────────────────────────────────────────────────────
# Act 2 — Proof: real files landed, doctor is green.
# ─────────────────────────────────────────────────────────────────────
Type "clear"
Enter
Sleep 400ms

Type "# Real files are on disk under .claude/."
Sleep 600ms
Enter
Sleep 300ms

Type "ls .claude/skills/ 2>/dev/null || ls .claude"
Enter
Sleep 3500ms

Type "# And the active plan is now part of the session context."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool plan show acme-billing-service | head -25"
Enter
Sleep 6000ms

Type "# doctor verifies installed items, registry health, and drift."
Sleep 700ms
Enter
Sleep 300ms

Type "skill-pool doctor"
Enter
Sleep 5500ms

# Hold the final frame so the GIF loop ends on a useful screen.
Sleep 2500ms
