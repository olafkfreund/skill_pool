# skill-pool — CLI showcase tape (v2: Projects + Plans)
#
# Renders the README/docs demo deterministically. Run with:
#
#   nix develop -c vhs scripts/demo.tape
#
# Produces docs/demo.webm and docs/demo.gif. The tape uses a scratch
# project under /tmp and an isolated XDG_CONFIG_HOME so it never
# touches the developer's real config. It assumes the local stack
# (server on http://127.0.0.1:8080, tenant `acme`) is running and
# that an API token has been minted via:
#
#   SKILL_POOL_DATABASE_URL=postgres://skillpool:skillpool@127.0.0.1:55432/skillpool \
#     ./target/debug/skill-pool-server admin token-create \
#     --tenant acme --name vhs-demo-v2 \
#     --scope "tenant:admin skills:read skills:publish"
#
# The recorded token below is the one minted for this tape run.

Output docs/demo.webm
Output docs/demo.gif

# `Require` is intentionally omitted — the recording explicitly
# prepends `target/release/` to PATH below so the tape can be
# rendered from any working directory the caller invokes vhs in.

Set Shell "bash"
Set FontSize 14
Set Width 1400
Set Height 760
Set Theme "Catppuccin Mocha"
Set TypingSpeed 28ms
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
# Ensure the locally-built skill-pool binary is on PATH for the
# duration of the recording.
Type "export PATH=/home/olafkfreund/Source/Calitii/skill_pool/target/release:$PATH"
Enter
Type "clear"
Enter
Type "rm -rf /tmp/sp-demo && mkdir -p /tmp/sp-demo && cd /tmp/sp-demo"
Enter
# Redirect XDG_CONFIG_HOME so `skill-pool login` saves credentials
# under /tmp/sp-demo instead of the developer's real ~/.config.
Type "export XDG_CONFIG_HOME=/tmp/sp-demo/xdg"
Enter
# Stand up a fake Rust project so `detect` finds Cargo.toml and
# `bootstrap` actually installs items. Fake the git remote too so
# `project show` / `doctor` light up.
Type "cargo init --name billing-service -q 2>/dev/null"
Enter
Type "git remote add origin https://github.com/acme/billing-service.git 2>/dev/null"
Enter
# Stage the Act 3 plan file now (hidden) so we don't have to do a
# mid-tape Hide/Show dance that occasionally leaks the command to
# the recording.
Type "printf '# Updated Plan\n\n## New Sprint\nProration logic shipping next week\n' > /tmp/sprint.md"
Enter
Type "clear"
Enter
Sleep 500ms
Show

# ─────────────────────────────────────────────────────────────────────
# Act 1 — Onboarding a new project (init → login → bootstrap)
# ─────────────────────────────────────────────────────────────────────
Type "# Onboard a new repo to its team's project on the registry."
Sleep 700ms
Enter
Sleep 400ms

Type "skill-pool init --project acme-billing-service"
Enter
Sleep 2800ms

Type "cat .skill-pool/manifest.toml"
Enter
Sleep 2800ms

Type "# Authenticate against the registry."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool login --registry http://127.0.0.1:8080 --tenant acme"
Enter
Sleep 1300ms
# `skill-pool login` reads the token from stdin via rpassword. VHS
# drives a real TTY so typing + Enter is exactly what a user would do.
Type "spk_df2b41d1487f20c4e31b130b9af5c419be66a8cf07e88441169c8ac7cb86d57b"
Enter
Sleep 1800ms

Type "# Bootstrap: pull the project's curated skills + agents."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool bootstrap --yes"
Enter
Sleep 7500ms

# ─────────────────────────────────────────────────────────────────────
# Act 2 — Discovering projects + viewing the active plan
# ─────────────────────────────────────────────────────────────────────
Type "clear"
Enter
Sleep 400ms

Type "# Which projects does the tenant publish?"
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool project list"
Enter
Sleep 4000ms

Type "skill-pool project show acme-billing-service"
Enter
Sleep 5500ms

Type "# Each project ships an active plan — the AI agent context."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool plan show acme-billing-service"
Enter
Sleep 9500ms

# ─────────────────────────────────────────────────────────────────────
# Act 3 — Importing a plan revision + verifying drift
# ─────────────────────────────────────────────────────────────────────
Type "clear"
Enter
Sleep 400ms

Type "# A curator imports a new plan revision from a markdown file."
Sleep 600ms
Enter
Sleep 300ms

Type "cat /tmp/sprint.md"
Enter
Sleep 2800ms

Type "skill-pool plan import acme-billing-service --file /tmp/sprint.md"
Enter
Sleep 3000ms

Type "skill-pool plan history acme-billing-service"
Enter
Sleep 5500ms

Type "# Finally — doctor checks drift, dangling symlinks, registry health."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool doctor"
Enter
Sleep 6500ms

# Final hold
Sleep 2500ms
