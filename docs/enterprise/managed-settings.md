# `managed-settings.json` — IT deploy guide

This guide is for IT admins rolling skill-pool to a fleet alongside
Claude Code. The artefact is the JSON file at
[`enterprise/managed-settings.json`](../../enterprise/managed-settings.json):
a single template that pins the tenant registry URL, the per-machine
token contract, an enterprise-defaults permission policy, and the
SessionStart / Stop hooks that wire `skill-pool` into every Claude Code
session on the box.

Anthropic's Claude Code reads `managed-settings.json` from a fixed
machine-wide path on each platform and merges it with the user's own
`~/.claude/settings.json`. The managed file wins for scalars (env,
`apiKeyHelper`) and concatenates for arrays (`additionalDirectories`,
`permissions.allow` / `permissions.deny`, `hooks`). That makes it the
right surface for organisation-wide policy: users cannot opt out, but
they can still add project-scoped settings on top.

## Why IT cares

- **One file = full skill-pool wiring.** Drop it once via MDM; every new
  Claude Code session on the box already knows the tenant, the registry
  URL, where to find the per-machine token, and which skills directory
  to scan.
- **Policy enforcement.** The `permissions.deny` block here is enforced
  regardless of what users or projects request. The destructive shell
  patterns, unsigned-pipe-to-shell installers, and outbound network
  tools are denied centrally.
- **Telemetry by default.** SessionStart runs `skill-pool ensure` which
  reconciles the project's skills against the registry; Stop runs
  `skill-pool capture-score` which persists a per-turn signal score that
  the Phase 4.6 capturer later picks up. Both feed `audit_events` on the
  registry, so SIEM fan-out (see [`audit-siem`](../../server/src/routes/audit_siem.rs))
  gets a complete picture.

## Three install paths

| OS         | Path                                                        | Mode | Owner          |
|------------|-------------------------------------------------------------|------|----------------|
| Linux / WSL | `/etc/claude-code/managed-settings.json`                   | 0644 | `root:root`    |
| macOS      | `/Library/Application Support/ClaudeCode/managed-settings.json` | 0644 | `root:wheel` |
| Windows    | `C:\ProgramData\ClaudeCode\managed-settings.json`           | n/a  | `Administrators` |

These paths come from Anthropic's Claude Code managed-settings docs.
Don't substitute user-scope paths (`~/Library/...`, `%APPDATA%`): those
are for user settings, not managed settings, and they don't enforce
across user accounts.

> **Heads-up — a previous draft of `docs/enterprise.md` listed
> `C:\Program Files\ClaudeCode\managed-settings.json` for Windows. The
> canonical machine-wide location is `C:\ProgramData\...` (matching the
> Linux `/etc/...` and macOS `/Library/...` pattern). The example
> Ansible / Intune snippets in `docs/enterprise.md` are scheduled for an
> update; new deploys should use the path in the table above.**

### Permissions, briefly

- Linux: `chown root:root` and `chmod 0644`. Don't world-write — Claude
  Code refuses to load managed settings off a writable-by-anyone file.
- macOS: same. Root-owned, group `wheel`, readable by everyone.
- Windows: ACL it `Administrators:Full`, `Authenticated Users:Read`.

## Section-by-section walkthrough

The template ships at `enterprise/managed-settings.json`. Below is a
guided tour; refer back to the file for exact field names.

### `_comment` (skill-pool extension)

A JSON array at the top of the file documenting every `<ANGLE_BRACKET>`
placeholder and the three install paths. It is **not** a Claude Code
managed-settings field — but JSON allows arbitrary extra keys, so the
file still parses cleanly and Claude Code ignores it. Strip it before
diffing against a previously-deployed copy:

```bash
jq 'del(._comment)' enterprise/managed-settings.json > /tmp/ms.json
diff /tmp/ms.json /etc/claude-code/managed-settings.json
```

### `apiKeyHelper`

```json
"apiKeyHelper": "/usr/local/bin/skill-pool-bootstrap"
```

`apiKeyHelper` is the Claude Code-blessed way to source the Anthropic
API key from a script rather than baking it into the file. Skill-pool's
[`enterprise/skill-pool-bootstrap.sh`](../../enterprise/skill-pool-bootstrap.sh)
reads `SKILL_POOL_TOKEN_FILE` (pinned in `env` below) and prints the
token on stdout. If your org has its own Anthropic helper, leave
`apiKeyHelper` pointing at theirs and use `skill-pool-bootstrap` only
via the env-vars below — both can coexist.

### `env`

```json
"env": {
  "SKILL_POOL_REGISTRY":   "<REGISTRY_BASE_URL>",
  "SKILL_POOL_TENANT":     "<TENANT_SLUG>",
  "SKILL_POOL_TOKEN_FILE": "/etc/skill-pool/token",
  "SKILL_POOL_AUDIT_URL":  "<REGISTRY_BASE_URL>/v1/tenant/audit-siem"
}
```

- `SKILL_POOL_REGISTRY` — tenant origin, no trailing slash. Same value
  the CLI uses everywhere; pinning it here means users can't redirect
  the CLI to a different (e.g. dev) tenant.
- `SKILL_POOL_TENANT` — tenant slug. Used by the CLI to disambiguate
  when one token has multi-tenant scope.
- `SKILL_POOL_TOKEN_FILE` — where the CLI looks for the per-machine
  registry token. Drop the token here as a 0600 file (root-owned).
- `SKILL_POOL_AUDIT_URL` — destination for any future hook-side audit
  POSTs. Today, audit events are written **server-side** by the
  registry on every mutating endpoint, then fanned out to your SIEM via
  the `audit-siem` config (`PUT /v1/tenant/audit-siem`, see
  [`audit_siem.rs`](../../server/src/routes/audit_siem.rs)). The env-var
  is set here so hooks (or a future `skill-pool audit-emit` subcommand)
  can post directly without needing additional config.

> **Upstream limitation (worked around):** Claude Code has no documented
> managed-settings key that POSTs audit events to an arbitrary URL — that
> would let IT route per-session telemetry to skill-pool's `/v1/tenant/audit-siem`
> directly. We work around it by wiring the **Stop hook** (`skill-pool
> capture-score`, defined further down) which writes per-session JSON to
> `~/.skill-pool/sessions/` and uploads to the registry on the next
> capturer-daemon tick. The registry then fans those events into the
> tenant's configured SIEM webhook (Splunk HEC, Datadog Logs, or generic
> POST — configured via `PUT /v1/tenant/audit-siem`). No data is lost; the
> path is just async rather than synchronous. If Anthropic later ships a
> first-class `auditEndpoint` key in managed-settings, this template will
> add it and the Stop-hook detour becomes redundant.

### `additionalDirectories`

```json
"additionalDirectories": [
  "<TENANT_LIBRARY_PATH>",
  "/var/lib/skill-pool/skills"
]
```

`additionalDirectories` extends the set of locations Claude Code scans
for skills/agents/commands. The first entry is the tenant-managed
library root (push from the registry via MDM, e.g.
`/var/lib/skill-pool/library/<TENANT_SLUG>`). The second is the legacy
system-wide skill drop that the existing
[`enterprise.rs`](../../server/src/routes/enterprise.rs) endpoint
already references — keep it in the list for backwards compatibility
with existing fleets.

### `permissions.allow` / `permissions.deny`

The `allow` list is intentionally narrow: shell commands for the
`skill-pool` and `skill-pool-server admin` CLIs, read-only git
introspection (`status`, `diff`, `log`, `branch`), plus the four
non-shell tools (`Read`, `Glob`, `Grep`, `Edit`) that every
non-destructive task needs.

`deny` is the policy backbone. It blocks:

- Destructive recursive deletes of `/`, `/*`, `~`, `$HOME`.
- Pipe-to-shell installers (`curl | sh`, `wget | bash`, both
  variants) — universally banned in regulated environments.
- Privilege escalation and lateral movement (`sudo`, `ssh`, `scp`,
  `rsync`).
- Outbound HTTP recon (`WebFetch`, `WebSearch`) — Claude Code's
  built-in tools, denied so off-fleet content can't enter prompts.

These are starting points. Extend `deny` for org-specific patterns
(e.g. `Bash(aws s3 rm *)`, `Bash(kubectl delete *)`). Remember that
`deny` wins over `allow` at every scope, so a user can't opt back into
something IT has denied.

### `hooks.SessionStart` and `hooks.Stop`

```json
"hooks": {
  "SessionStart": [
    { "matcher": "*", "hooks": [ { "type": "command", "command": "skill-pool ensure --quiet", "timeout": 30 } ] }
  ],
  "Stop": [
    { "matcher": "*", "hooks": [ { "type": "command", "command": "skill-pool capture-score", "timeout": 10 } ] }
  ]
}
```

Both hooks are exactly what the project-scope `skill-pool hook-install`
CLI writes (see [`cli/src/cmd/hook_install.rs`](../../cli/src/cmd/hook_install.rs)).
Hoisting them into managed settings is what makes them fleet-wide and
non-removable by users.

- **SessionStart** keeps the project's skills installed against the
  tenant registry — covers users who don't use direnv.
- **Stop** runs the Phase 4.5 signal scorer, writing per-turn scores to
  `~/.skill-pool/sessions/`. The Phase 4.6 capturer (see
  [`nix/modules/skill-pool-capturer.nix`](../../nix/modules/skill-pool-capturer.nix))
  picks those up on its hourly timer and turns them into drafts that
  curators review on the registry.

## Verify it landed

After the MDM push completes on a target machine, run the following
from a normal user shell:

1. **Confirm Claude Code sees the file.**
   ```bash
   claude --version
   # then in any session:
   /doctor   # surfaces the managed-settings path and parse status
   ```
   If Claude Code can't parse it, `/doctor` says so and falls back to
   user settings only — fix the JSON and redeploy.

2. **Confirm the env-vars are pinned.**
   ```bash
   claude --print 'echo $SKILL_POOL_REGISTRY $SKILL_POOL_TENANT'
   ```
   You should see the registry URL and tenant slug from the template.

3. **Confirm `skill-pool` runs.** Open a session in a project with a
   known skill, ask Claude to use it, and watch
   `~/.skill-pool/sessions/` accumulate a per-turn score JSON.

4. **Confirm the audit row landed on the registry.** Either tail the
   tenant SIEM stream (if `PUT /v1/tenant/audit-siem` is configured) or
   query the registry DB directly:
   ```sql
   SELECT action, target_kind, ts
     FROM audit_events
    WHERE tenant_id = '<TENANT_ID>'
    ORDER BY ts DESC LIMIT 20;
   ```
   You should see `skills.get`, `drafts.create`, or similar within a
   minute of the user finishing a turn.

## Removing or rotating

`managed-settings.json` is idempotent — re-running your MDM pipeline
just overwrites the file. To temporarily disable the integration
without uninstalling, deploy a copy with `hooks: {}` and
`env: { ...same minus SKILL_POOL_REGISTRY }`; the CLI then errors out
cleanly and Claude Code sessions are unaffected.

To rotate the token, push a new value to `/etc/skill-pool/token`
(0600). No managed-settings change is needed because the file path is
already pinned.

## References

- [`enterprise/managed-settings.json`](../../enterprise/managed-settings.json)
  — the template to deploy.
- [`enterprise/install.sh`](../../enterprise/install.sh) — pulls a
  tenant-tailored copy from the registry and installs it to the
  OS-appropriate path.
- [`docs/enterprise.md`](../enterprise.md) — fleet rollout (Ansible /
  Jamf / Intune) and token-rotation procedures.
- [`server/src/routes/enterprise.rs`](../../server/src/routes/enterprise.rs)
  — the registry endpoint that serves this template per tenant.
- Anthropic Claude Code managed-settings docs — canonical schema for
  `apiKeyHelper`, `env`, `additionalDirectories`, `permissions`, and
  `hooks`. (Refer to the version your fleet runs; field names have been
  stable across recent Claude Code releases.)
