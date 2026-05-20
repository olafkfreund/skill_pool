# skill-pool — CLI showcase tape
#
# Renders the README/docs demo deterministically. Run with:
#
#   nix develop -c vhs scripts/demo.tape
#
# This produces docs/demo.webm and docs/demo.gif. The tape uses a
# scratch project under /tmp and an isolated SKILL_POOL_CONFIG so it
# never touches the developer's real config. It assumes the local
# stack (server on http://127.0.0.1:8080, tenant `acme`) is running
# and that a working API token has been minted — see
# scripts/demo.README for how the demo token is seeded. If the token
# is missing the tape still finishes, but `search` / `add` will show
# an auth error instead of a result table.

Output docs/demo.webm
Output docs/demo.gif

# `Require` is intentionally omitted — the recording explicitly
# prepends `target/release/` to PATH below so the tape can be
# rendered from any working directory the caller invokes vhs in.

Set Shell "bash"
Set FontSize 14
Set Width 1200
Set Height 720
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
# Ensure the locally-built skill-pool binary is on PATH for the
# duration of the recording. The CI/render entry-point can also
# build a Nix-packaged binary into PATH and skip this line.
Type "export PATH=/home/olafkfreund/Source/Calitii/skill_pool/target/release:$PATH"
Enter
Type "clear"
Enter
Type "rm -rf /tmp/sp-demo && mkdir -p /tmp/sp-demo && cd /tmp/sp-demo"
Enter
# Redirect XDG_CONFIG_HOME so `skill-pool login` saves credentials
# under /tmp/sp-demo instead of the developer's real ~/.config.
# (Config::save() always uses the default path, so SKILL_POOL_CONFIG
# alone is not enough — see cli/src/config.rs.)
Type "export XDG_CONFIG_HOME=/tmp/sp-demo/xdg"
Enter
Type "cargo init --name demo-app -q 2>/dev/null"
Enter
Type "clear"
Enter
Sleep 500ms
Show

# --- Act 1: a fresh Rust project meets skill-pool ---
Type "# A fresh Rust project — let's onboard it to the team's skill registry."
Sleep 600ms
Enter
Sleep 400ms

Type "ls"
Enter
Sleep 1200ms

Type "skill-pool init"
Enter
Sleep 1800ms

Type "cat .skill-pool/manifest.toml"
Enter
Sleep 2000ms

# --- Act 2: point the CLI at the registry ---
Type "# Authenticate against the registry. Token comes from the portal."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool login --registry http://127.0.0.1:8080 --tenant acme"
Enter
Sleep 1200ms
# `skill-pool login` reads the token from stdin via rpassword. VHS
# drives a real TTY so typing + Enter is exactly what a user would do.
Type "spk_demo_9901204c7a3d6782e8f5007f7d4aa940"
Enter
Sleep 1800ms

# --- Act 3: detect stack ---
Type "# Detect the stack from filesystem fingerprints — no LLM, no network."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool detect"
Enter
Sleep 2500ms

# --- Act 4: search the catalog ---
Type "# Browse what the team has published."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool search"
Enter
Sleep 2800ms

Type "skill-pool search axum"
Enter
Sleep 2500ms

# --- Act 5: install a skill ---
Type "# Install one into .claude/skills/ and the manifest."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool add a11y-audit"
Enter
Sleep 2500ms

Type "ls .claude/skills/"
Enter
Sleep 1800ms

# --- Act 6: health check ---
Type "# doctor: drift, dangling symlinks, registry reachability."
Sleep 600ms
Enter
Sleep 300ms

Type "skill-pool doctor"
Enter
Sleep 2500ms

# Final hold
Sleep 1500ms
