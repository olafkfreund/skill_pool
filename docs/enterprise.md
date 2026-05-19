# Enterprise deployment — Claude Enterprise + skill-pool

This guide is for IT admins rolling skill-pool to a fleet alongside Claude
Code. The integration is **additive** with Anthropic's own enterprise
tooling — your existing managed-settings.json keeps working, and skill-pool's
config merges in.

## What gets pushed to each machine

1. **`managed-settings.json`** — Claude Code's system-wide settings. We pin
   `env.SKILL_POOL_*` plus a baseline permissions list.
2. **`/etc/skill-pool/token`** — a per-tenant API token consumed by the
   skill-pool CLI on every invocation. 0600 perms. Rotated via MDM.
3. **`/usr/local/bin/skill-pool`** + **`/usr/local/bin/skill-pool-bootstrap`**
   — the CLI binary and the helper script that resolves the token from
   `SKILL_POOL_TOKEN_FILE`.

That's everything. No agent on the box; the CLI invokes the registry over
HTTPS on demand.

## OS paths

| OS | `managed-settings.json` |
|---|---|
| macOS | `/Library/Application Support/ClaudeCode/managed-settings.json` |
| Linux / WSL | `/etc/claude-code/managed-settings.json` |
| Windows | `C:\ProgramData\ClaudeCode\managed-settings.json` |

> For a deployable template plus a section-by-section walkthrough of
> every block (`apiKeyHelper`, `env`, `additionalDirectories`,
> `permissions`, `hooks`), see
> [`docs/enterprise/managed-settings.md`](enterprise/managed-settings.md)
> and the JSON at [`enterprise/managed-settings.json`](../enterprise/managed-settings.json).

## Bootstrap (1 minute, one machine)

```bash
# Mint an admin token if you don't have one yet.
skill-pool-server admin token-create \
  --tenant acme --name mdm-bootstrap --scope 'tenant:admin'

# Run the installer on the target box.
sudo enterprise/install.sh \
  --registry https://acme.skill-pool.example.com \
  --tenant acme \
  --admin-token spk_... \
  --token-out /etc/skill-pool/token
```

The installer downloads a tenant-tailored `managed-settings.json` from
the registry, validates the JSON parses, and writes it to the
OS-appropriate path. With `--token-out`, it also drops the admin token
to a 0600 file the CLI can read at runtime.

> **Production note:** the admin token from `--admin-token` is what the
> installer uses to download the template. The token at `--token-out`
> doesn't have to be the same one — most orgs ship a separate, lower-
> scope token to the fleet. Mint one with `--scope 'skills:read'` or
> `'skills:read skills:publish'` and pass it via `--token-out` while
> keeping the admin token off the boxes.

## Fleet rollout — Ansible

```yaml
# roles/skill-pool/tasks/main.yml
- name: install skill-pool CLI
  ansible.builtin.get_url:
    url: "https://github.com/olafkfreund/skill_pool/releases/latest/download/skill-pool-linux-amd64"
    dest: /usr/local/bin/skill-pool
    mode: '0755'

- name: install bootstrap helper
  ansible.builtin.copy:
    src: skill-pool-bootstrap.sh
    dest: /usr/local/bin/skill-pool-bootstrap
    mode: '0755'

- name: ensure skill-pool config dir
  ansible.builtin.file:
    path: /etc/skill-pool
    state: directory
    mode: '0755'

- name: deploy per-machine token
  ansible.builtin.copy:
    content: "{{ skill_pool_runtime_token }}\n"   # vault-encrypted variable
    dest: /etc/skill-pool/token
    owner: root
    mode: '0600'
  no_log: true

- name: download managed-settings.json
  ansible.builtin.uri:
    url: "{{ skill_pool_registry }}/v1/enterprise/managed-settings"
    method: GET
    headers:
      Authorization: "Bearer {{ skill_pool_admin_token }}"
      X-Skill-Pool-Tenant: "{{ skill_pool_tenant }}"
    return_content: true
    status_code: 200
    dest: /etc/claude-code/managed-settings.json
    mode: '0644'
  no_log: true
```

Vault both tokens. The admin token is sensitive (it can mint other
tokens); the runtime token is one rotation away from being public anyway.

## Fleet rollout — Jamf (macOS)

1. Wrap `enterprise/install.sh` as a Policy script in Jamf Pro.
2. Pass `--registry`, `--tenant`, and `--admin-token` as Parameters 4/5/6.
3. Scope the policy to your dev population.
4. Trigger on Enrollment Complete + Recurring Check-in for drift.

For rotation: schedule a daily policy that re-runs the installer; the
managed-settings.json overwrite is idempotent.

## Fleet rollout — Intune

PowerShell rolls out the same payload on Windows:

```powershell
# (Sketch — the install.sh equivalent for Windows lands when there's demand.)
Invoke-WebRequest -Uri "$Registry/v1/enterprise/managed-settings" `
  -Headers @{ Authorization = "Bearer $AdminToken"; "X-Skill-Pool-Tenant" = $Tenant } `
  -OutFile "$env:ProgramFiles\ClaudeCode\managed-settings.json"
```

## How `managed-settings.json` composes with Anthropic's

Claude Code merges managed settings with user settings (`~/.claude/settings.json`)
and project settings (`.claude/settings.json`). Conflict resolution favours the
narrower scope for arrays (`additionalDirectories` concatenates) and the broader
scope for scalars. Bottom line:

- Skill-pool's `env.SKILL_POOL_*` cannot be overridden by users.
- Skill-pool's `permissions.allow` adds to what Anthropic's settings allow.
- Skill-pool's `permissions.deny` is enforced regardless of project settings.

If you maintain a separate Anthropic-side managed-settings.json (e.g.
for the Anthropic API key), keep both files in version control; skill-pool's
endpoint generates **only the skill-pool slice**.

## Token rotation

1. Mint a fresh runtime token: `skill-pool-server admin token-create --tenant acme --name fleet-2026Q2 --scope skills:read`
2. Push the new token to `/etc/skill-pool/token` via your existing MDM.
3. After confirming the fleet is on the new token, revoke the old one:
   ```sql
   UPDATE tenant_api_tokens SET revoked_at = now() WHERE id = '...';
   ```
   (Admin-CLI command for revocation lands in a future iteration of #8.)

## Audit

Every download of `managed-settings.json` lives in `audit_events` with
the admin token's `actor_token` field set. Filter SIEM for
`action = 'enterprise.managed_settings_download'` if your retention
policy requires per-pull tracing. (The audit write itself lands alongside
the SAML ACS work; for now downloads are not audited — track via the
admin token's `last_used_at` instead.)
