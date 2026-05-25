# Onboarding showcase

A two-half walkthrough of how a project gets onto skill-pool. Both halves
share the same fixture data (`acme` tenant, `acme-billing-service`
project) so the story reads as one continuous flow:

1. **Half A — Developer onboards a fresh repo from the CLI.** They run
   three commands (`init`, `login`, `bootstrap`) and end up with the
   team's curated skill bundle on disk under `.claude/skills/`, plus the
   active project plan in their session context.
2. **Half B — Curator manages the same project in the portal.** They see
   what bundle is pinned to `acme-billing-service`, browse the curated
   items, and read the active plan that the developer's `bootstrap`
   just pulled.

## Half A — CLI onboarding (~60s)

What the developer types after `git clone`-ing the repo:

```bash
$ skill-pool init --project acme-billing-service
$ skill-pool login --registry http://… --tenant acme
$ skill-pool bootstrap --yes
$ ls .claude/skills/        # real files on disk
$ skill-pool doctor         # everything green
```

<video src="onboarding-cli.webm" controls width="800" loop muted playsinline>
  Your browser does not render WebM inline. See the GIF below.
</video>

![CLI onboarding (gif fallback)](onboarding-cli.gif)

Source tape: [`scripts/demo-onboarding-cli.tape`](../../scripts/demo-onboarding-cli.tape).

## Half B — Portal configuration (~20s)

The same project, seen from the curator's side:

1. `/admin/projects` — list of curated bundles for this tenant
2. Click **Edit** on `acme-billing-service` → detail view
3. Browse the curated items (skills + agents + commands) the developer
   just installed
4. Read the active plan that lives at the bottom of the page
5. Back to the list

<video src="onboarding-portal.webm" controls width="800" loop muted playsinline>
  Your browser does not render WebM inline. See the GIF below.
</video>

![portal configuration (gif fallback)](onboarding-portal.gif)

Source spec: [`scripts/playwright/onboarding-portal.spec.ts`](../../scripts/playwright/onboarding-portal.spec.ts).

## Run it yourself

Both halves record deterministically from a single command. Prereqs:

- The portal is running locally (e.g. via `docker compose -f server/compose.yaml up -d`
  or the NixOS module). Health-checked at `http://127.0.0.1:8080/v1/healthz`.
- `scripts/seed-demo.sh` has been run once to mint an admin token at
  `~/.config/skill-pool/config.toml`.
- `vhs`, `ffmpeg`, and `google-chrome` are on PATH (the nix devshell
  provides `vhs` and `ffmpeg`; install `google-chrome` via
  `environment.systemPackages` on NixOS or your distro's package manager
  elsewhere).

Then:

```bash
./scripts/record-onboarding-demo.sh
```

Which:

1. Verifies fixture state (`scripts/seed-demo-onboarding.sh` — imports an
   active plan on `acme-billing-service` if none exists)
2. Renders the CLI half via `vhs scripts/demo-onboarding-cli.tape`
3. Records the portal half via Playwright (`scripts/playwright/`), driving
   the system `google-chrome` against `http://127.0.0.1:3030`
4. Converts the portal webm to gif via ffmpeg (palettegen + bayer dither)
5. Drops all four files into `docs/demo/` next to this README

Total wall-clock: ~3 minutes.

## Recording notes

- The CLI tape uses an isolated `XDG_CONFIG_HOME=/tmp/sp-onboard/xdg` so
  it never touches the developer's real `~/.config/skill-pool/`.
- The Playwright spec authenticates by setting the `sp_token` +
  `sp_tenant` cookies before its first navigation. The token comes from
  `~/.config/skill-pool/config.toml` (or `$SP_DEMO_TOKEN`).
- Playwright uses the system-installed `google-chrome` rather than its
  bundled chromium because the chromium binary expects FHS-style library
  paths (`libnspr4.so` etc.) and won't launch on NixOS without a
  `buildFHSUserEnv` wrapper.
- Re-recording wipes `docs/demo/onboarding-*.{webm,gif}` in place. Commit
  the new media files alongside any code changes that motivated the
  re-record.
