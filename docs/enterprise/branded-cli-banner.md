# Branded CLI banner

> Enterprise tier. Closes one of the boxes on issue [#9](https://github.com/calitii/skill-pool/issues/9).

A per-tenant one-line greeting that the `skill-pool` CLI prints to
**stderr** the first time it runs in any shell session in a 24-hour
window. Useful for:

- Reminding engineers which tenant's registry they're talking to
  (especially when they bounce between `acme` and `acme-staging`).
- Pointing at an internal portal: "Acme skill registry — internal docs
  at https://wiki.acme.example.com/skills".
- One-time announcements: "Maintenance window Saturday 02:00 UTC".

What it is **not**: a notification channel, a full MOTD, or an
unconditional "every command shouts at you" banner. It's deliberately
small.

---

## What you get

```
$ skill-pool ensure
Acme skill registry — internal docs below.
https://wiki.acme.example.com/skills
installed 4 skills
```

- One line of free-form text (≤ 240 characters).
- Optionally one URL on the line below it (must be `https://`).
- That's it. No ASCII art, no boxes, no terminal hyperlinking — OSC-8
  rendering varies across iTerm2 / Alacritty / WezTerm / VS Code's
  integrated terminal, so we print the URL plain. Most modern
  terminals make it click-to-open on their own.

---

## Setting the banner

### Via the admin CLI

```bash
# Greeting + URL.
skill-pool-server admin tenant-banner-set \
    --slug acme \
    --text "Acme skill registry — internal docs below." \
    --url  "https://wiki.acme.example.com/skills"

# Greeting only.
skill-pool-server admin tenant-banner-set \
    --slug acme \
    --text "Maintenance window Saturday 02:00 UTC"

# Update just the URL, leave text alone.
skill-pool-server admin tenant-banner-set \
    --slug acme \
    --url "https://status.acme.example.com"

# Clear one column (empty string sentinel).
skill-pool-server admin tenant-banner-set \
    --slug acme \
    --url ""

# Clear both.
skill-pool-server admin tenant-banner-set --slug acme --clear
```

### Via SQL (advanced / migration scripts)

```sql
UPDATE tenants
   SET banner_text = 'Acme skill registry — see internal docs below.',
       banner_url  = 'https://wiki.acme.example.com/skills'
 WHERE slug = 'acme';
```

The DB CHECK constraints will reject:
- `banner_text` longer than 240 characters or empty.
- `banner_url` that isn't `https://...` or contains whitespace.

---

## Reading the banner (debugging)

```bash
curl -H "x-skill-pool-tenant: acme" https://registry.acme.example.com/v1/tenant/profile/banner
# → {"text":"Acme skill registry — internal docs below.","url":"https://wiki.acme.example.com/skills"}
```

No auth required — the banner is policy, not a secret.

---

## When the CLI prints it

The banner is shown **only when all of these are true**:

| Condition | Why |
|---|---|
| `stdout` is a TTY | Don't poison pipelines (`skill-pool search foo \| grep bar`) or CI logs. |
| `SKILL_POOL_NO_BANNER` env is unset | Operator opt-out for shared automation accounts. |
| `~/.skill-pool/banner-shown` is older than 24h (or missing) | Once-per-day per machine, not once-per-command. |
| A registry is configured | Nothing to fetch otherwise. |

If the registry is slow, the fetch times out after 1.5 seconds and the
CLI proceeds without the banner. **The banner never blocks or fails a
command.**

---

## Suppression

- One-off: `SKILL_POOL_NO_BANNER=1 skill-pool ensure`
- Permanent (per shell): add `export SKILL_POOL_NO_BANNER=1` to your
  `~/.bashrc` / `~/.zshrc`.
- Per-machine: `touch ~/.skill-pool/banner-shown` and edit its mtime to
  the future (`touch -d '2099-01-01' ~/.skill-pool/banner-shown`).

---

## Caveats

- The URL is **not** auto-opened. Most terminals will make it
  clickable; we deliberately don't emit OSC-8 hyperlinks because their
  rendering is inconsistent across terminals and can produce ugly
  `\e]8;;URL\e\\...` escapes in terminals that don't support them.
- The banner is per-tenant, not per-user. If you want different
  messages for different roles, use a real notification system.
- The 24h dedup window is not configurable. If you need an
  "announcement" pattern (force-show on next CLI run), bump the banner
  text and tell users to `rm ~/.skill-pool/banner-shown`.
- The banner is fetched fresh each time we decide to show it — we do
  not cache the response body. An admin who clears the banner sees the
  effect propagate on the next 24h-window expiry per user.
