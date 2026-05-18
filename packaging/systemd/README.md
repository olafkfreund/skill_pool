# systemd user units for skill-pool

## What's here

- `skill-pool-capturer.service` — oneshot LLM pipeline that drafts
  skills from draft-worthy sessions. Calls the Anthropic API + your
  registry's `/v1/drafts`.
- `skill-pool-capturer.timer` — fires the service hourly with jitter.

## Install

```bash
mkdir -p ~/.config/systemd/user/
cp packaging/systemd/skill-pool-capturer.{service,timer} ~/.config/systemd/user/
systemctl --user daemon-reload
systemctl --user enable --now skill-pool-capturer.timer
```

## Required environment

The unit reads these from your user session (set them in
`~/.config/environment.d/skill-pool.conf` or your shell init):

```
ANTHROPIC_API_KEY=sk-ant-...
SKILL_POOL_REGISTRY=https://acme.skill-pool.example.com
```

For the registry token, `skill-pool login --registry … --tenant …` (run
once interactively) persists a token under `~/.config/skill-pool/`. The
unit picks it up automatically — no env var needed.

## Inspect

```bash
# When does it fire next?
systemctl --user list-timers skill-pool-capturer.timer

# Last run output:
journalctl --user -u skill-pool-capturer.service -n 200

# Run it now without waiting for the timer:
systemctl --user start skill-pool-capturer.service
```

## Disable

```bash
systemctl --user disable --now skill-pool-capturer.timer
```

The score store at `~/.skill-pool/sessions/` is preserved — re-enabling
the timer picks up where you left off.

## On NixOS

The flake exposes the units; on a NixOS machine you can wire them via
`systemd.user.services` and `systemd.user.timers` instead of copying
files. A `nixosModule` is on the Phase 5 roadmap.
