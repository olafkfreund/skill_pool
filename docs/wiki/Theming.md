# Theming

> Per-tenant brand identity: palette, logo, favicon, font picker,
> optional custom-CSS overlay. Every theme save runs WCAG AA contrast
> validation; bad palettes are rejected at the UI and at the API.

![Theme admin page](https://raw.githubusercontent.com/olafkfreund/skill_pool/main/docs/images/theme.webp)

## What you can change

Eight color slots, one border-radius, one brand name, one logo, one
favicon, one font family, optional custom CSS overlay, optional
"Powered by skill-pool" footer toggle. The full set drives the
`--sp-*` CSS custom properties consumed by every Svelte component.

### Color slots

| CSS variable | Schema column | Used for |
|---|---|---|
| `--sp-primary`    | `primary_`    | Primary button bg, CTA links |
| `--sp-primary-fg` | `primary_fg`  | Text on top of `--sp-primary` |
| `--sp-accent`     | `accent`      | Badge backgrounds, secondary highlights |
| `--sp-bg`         | `bg`          | Page background (`<body>`) |
| `--sp-fg`         | `fg`          | Body text |
| `--sp-muted`      | `muted`       | Card surfaces, tag chip backgrounds |
| `--sp-muted-fg`   | `muted_fg`    | Secondary / supporting text |
| `--sp-border`     | `border`      | Divider lines, input/card outlines |
| `--sp-radius`     | `radius`      | Border-radius for buttons, cards, inputs |
| `--sp-font-family`| `font_family` | Resolved Google Fonts stack |

`logo_uri`, `logo_storage_key`, `brand_name`, and `footer_branding`
don't have CSS variables — they're consumed directly by Svelte
templates.

## Defaults

The default palette is duplicated in three places — API, web, SQL
migration — and verified by an in-server test
(`server/src/routes/theme.rs::tests::validates_default_theme`):

| Field | Default |
|---|---|
| `brand_name` | (tenant slug) |
| `primary` | `#2563eb` (blue-600) |
| `primary_fg` | `#ffffff` |
| `accent` | `#0ea5e9` (sky-500) |
| `bg` | `#ffffff` |
| `fg` | `#0f172a` (slate-900) |
| `muted` | `#f1f5f9` (slate-100) |
| `muted_fg` | `#475569` (slate-600) |
| `border` | `#e2e8f0` (slate-200) |
| `radius` | `0.5rem` |
| `footer_branding` | `true` |

All defaults pass every WCAG AA pair check:

- `fg` on `bg` = **18.10:1** (AA, AAA-clear)
- `primary_fg` on `primary` = **5.17:1** (AA-clear)
- `muted_fg` on `muted` = **6.79:1**
- `muted_fg` on `bg` = **7.43:1**

## WCAG AA contrast validation

Every theme save runs `checkThemeContrast` in
`web/src/lib/contrast.ts` and refuses to call the API if any pair
fails. The server then runs its own body-text-only check inside
`validate` in `server/src/routes/theme.rs` and returns HTTP 400 if
it disagrees.

The four pairs:

| Pair | Required ratio | Why |
|---|---|---|
| `fg` / `bg` | **4.5:1** (AA, body) | Page-level body text |
| `primaryFg` / `primary` | **3.0:1** (AA, UI) | Button label on CTA color |
| `mutedFg` / `muted` | **4.5:1** (AA, body) | Secondary text on cards |
| `mutedFg` / `bg` | **4.5:1** (AA, body) | Secondary text on page bg |

What happens on save when a pair fails:

1. The admin theme page computes badges live as the color pickers
   change. Save is disabled while any pair fails (`disabled` attribute
   blocks the request from leaving the browser).
2. If a request *does* reach the server (e.g. an API client bypassing
   the UI), the form action re-runs `checkThemeContrast` and returns
   `fail(422, { contrastFailures, draft })`. The page renders a red
   "Save blocked" banner.
3. The server-side validator runs the fg/bg pair check on `PUT
   /v1/theme` and returns 400 with the actual ratio.

The badges next to each pair use four levels:

| Level | Threshold | Color |
|---|---|---|
| `AAA` | ≥ 7.0:1 | emerald |
| `AA` | ≥ 4.5:1 | sky |
| `AA-large` | ≥ 3.0:1 | amber (large text or UI components only) |
| `fail` | < 3.0:1 | red |

> **Known asymmetry.** The server-side validator only checks the
> fg/bg pair. The other three pairs are checked by the portal only.
> A direct API caller (curl, CI) can save a theme that fails the UI
> checks. Follow-up tracked.

## Schema

`tenant_theme` row (migrations `0002`, `0016`, `0020`):

| Column | Type | Nullable | Notes |
|---|---|---|---|
| `tenant_id` | UUID | NO | FK → tenants(id) ON DELETE CASCADE |
| `brand_name` | TEXT | NO | API caps to 1..=80 chars |
| `primary_` | TEXT | NO | Hex color, CHECK `~ '^#[0-9A-Fa-f]{3,8}$'` |
| `primary_fg` | TEXT | NO | Hex color |
| `accent` | TEXT | NO | Hex color |
| `bg` | TEXT | NO | Hex color |
| `fg` | TEXT | NO | Hex color |
| `muted` | TEXT | NO | Hex color |
| `muted_fg` | TEXT | NO | Hex color |
| `border` | TEXT | NO | Hex color |
| `radius` | TEXT | NO | CSS length string (not parsed) |
| `logo_uri` | TEXT | YES | External URL (honour-system, server doesn't fetch) |
| `footer_branding` | BOOLEAN | NO | Default TRUE |
| `logo_storage_key` | TEXT | YES | Object-store key |
| `logo_content_type` | TEXT | YES | CHECK `IN ('image/svg+xml', 'image/png', 'image/jpeg', 'image/webp')` |
| `logo_bytes_size` | INTEGER | YES | CHECK `0..=262144` (256 KiB) |
| `updated_at` | TIMESTAMPTZ | NO | Trigger-updated |

The three `logo_*` columns share an all-NULL-or-all-NOT-NULL CHECK
constraint so the GET endpoint always sees the content-type and
storage key together.

## Request-time resolution

Theme lookup happens once per HTTP request in
`web/src/hooks.server.ts`:

1. `resolveTenant(url, host)` derives the tenant slug from the URL
   (`?tenant=` wins for previews), then the leftmost Host label
   (`acme.example.com` → `acme`). Bare hostnames, `www`, IPv4
   literals, and `*.lan`/`*.local`/`*.nip.io` fall back to
   `SP_DEFAULT_TENANT` env, then to `"default"`.
2. `themeFor(slug)` calls `GET /v1/theme` (no auth — the login page
   needs branding before sign-in).
3. If the API responds, the result becomes the page tree's `Theme`.
   If the call throws or the tenant has no row, returns
   `{ ...DEFAULT_THEME, brandName: slug }` — the slug becomes the
   visible brand name until an admin saves a real one.
4. The resolved `Theme` is stashed on `event.locals.theme`.
5. The root layout wraps `themeToCss(data.theme)` in a `:root { … }`
   block inside `<svelte:head>`. Source order wins over the seed
   `app.css` block.

Three consequences:

- Theme is computed on **every** request — no caching layer between
  API and SSR. (The API itself can cache its DB read.)
- Anonymous routes (login, public catalog) carry the theme
  identically to authed routes.
- If the API is unreachable, the portal still renders with default
  branding.

## Logo upload + sanitization

| Method | Path | Auth | Body |
|---|---|---|---|
| GET | `/v1/theme/logo` | None | — |
| POST | `/v1/theme/logo` | `tenant:admin` | multipart/form-data with one `file` part |
| DELETE | `/v1/theme/logo` | `tenant:admin` | — |

**Accepted content types:** `image/svg+xml`, `image/png`,
`image/jpeg`, `image/webp`. Size cap: **256 KiB**, enforced at three
layers (DB CHECK, multipart handler, Axum body-limit middleware).

**SVG reject list** (`logo_sanitize::sanitize_svg`):

- Element openers — `<script`, `</script`, `<foreignobject`,
  `<iframe`, `<embed`, `<object`, `<base`, `<link`, `<meta`.
- Dangerous URI schemes — `javascript:`, `vbscript:`,
  `data:text/html`, `data:application/javascript`,
  `data:text/javascript`.
- CSS escapes — `expression(`, `@import`, `behavior:`.
- Inline event handlers — any `on<letters>=` attribute (caught by
  the hand-rolled state machine; whitespace before `=` is tolerated
  as a bypass shape and still rejected).
- Off-origin `href` / `xlink:href` — must be empty or a same-document
  fragment (`#…`).

After the deny pass, SVG is parsed by `quick-xml` and re-emitted
without comments or processing instructions. CDATA blocks are
re-escaped as text. Malformed XML is rejected outright.

**Raster formats are NOT re-encoded** — see the head comment in
`server/src/logo_sanitize.rs` for the threat-model reasoning. The
server checks magic bytes:

- PNG: `89 50 4E 47 0D 0A 1A 0A`
- JPEG: `FF D8 FF`
- WEBP: `RIFF` + 4 bytes + `WEBP`

Any mismatch returns 400.

**Storage key shape.** `{tenant_id}/theme/logo.{ext}` where `ext` is
one of `svg`, `png`, `jpg`, `webp`. Re-upload of the same format
overwrites in place. Format change writes a new key — the upload
issues a best-effort delete of the previous key.

**Cache headers.** `GET /v1/theme/logo` returns `Cache-Control:
public, max-age=300` (five minutes). Long enough to dodge login-page
load, short enough that a logo replace shows up across the org
within minutes.

## Favicon

Tenant admins may upload an explicit favicon at
`POST /v1/theme/favicon`. When no favicon is uploaded but a logo is,
`GET /v1/theme/favicon` transparently serves the logo bytes — so every
browser tab still shows the tenant's brand mark.

| Method | Path | Auth | Notes |
|---|---|---|---|
| POST | `/v1/theme/favicon` | `tenant:admin` | Multipart |
| DELETE | `/v1/theme/favicon` | `tenant:admin` | 204 |
| GET | `/v1/theme/favicon` | public | Falls back to logo bytes when unset |

Accepted formats: same as logo plus `image/x-icon` (Windows ICO).
ICO files validated to have a proper ICONDIR header. SVG runs the
same sanitizer. **Size cap: 64 KiB** (tighter than logo).

## Font picker

Curated 12-entry Google Fonts allowlist in
`server/src/routes/theme.rs::ALLOWED_FONTS`:

| Family | Style | Why |
|---|---|---|
| `system` | OS-native | zero network cost; default |
| `Inter` | Sans | de-facto standard for dashboards |
| `IBM Plex Sans` | Sans | data-dense tables |
| `JetBrains Mono` | Mono | code-heavy UIs |
| `Source Sans 3` | Sans | neutral |
| `Source Serif 4` | Serif | editorial |
| `Merriweather` | Serif | screen-optimized body text |
| `Roboto` | Sans | recognition |
| `Fira Sans` | Sans | humanist tone |
| `Atkinson Hyperlegible` | Sans | accessibility-first |
| `Work Sans` | Sans | modern grotesque |
| `Lora` | Serif | contemporary long-form |

Every entry is OFL or Apache 2.0 — self-hosting permitted.

When a tenant picks a non-`system` family, the portal injects a
`<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=...">`
and sets `--sp-font-family` on `:root`.

API:

- `GET /v1/theme/fonts` — public. Returns `{ "allowed": [...] }`.
- `PUT /v1/theme` — accepts `font_family` (optional). Values not in
  the allowlist are rejected with 400.

## Custom CSS overlay

Per-tenant custom CSS upload (Phase 2 / #9). Subject to strict
sanitization:

- No `@import` (would let a tenant pull in arbitrary external CSS).
- No `behavior:` (legacy IE CSS expression).
- No `expression(...)` (CSS-side JS).
- No `url(javascript:...)` (URL scheme deny).
- No `<` chars (defense against script-tag injection in CSS).

The CSS gets injected into a `<style>` block in `<svelte:head>` AFTER
the theme block, so it can override individual properties. CSP is
respected — the page's `Content-Security-Policy: style-src 'self'
'unsafe-inline'` already allows inline styles for the theme block, so
the overlay piggy-backs on the same policy.

Full detail: `docs/enterprise/custom-css.md`.

## Branded emails

Once a tenant configures SMTP via `admin tenant-email-config`, the
same `brand_name` is used in transactional email headers. See
`docs/enterprise/branded-emails.md`.

## Footer branding toggle

`footer_branding` boolean controls whether the site footer renders a
"Powered by skill-pool" credit. Default `TRUE`. Any caller with
`tenant:admin` scope can flip it.

Tier gating: none today — every tier can turn the footer off. A
future stretch is to lock it ON for the Free tier.

## Open Graph images

Per-tenant OG image generator at
`/v1/og?slug=<skill-slug>` (Phase 2 / #9). Renders a 1200x630 PNG
with the skill's name and the tenant's brand mark for social sharing.
See `docs/enterprise/og-images.md`.

## Limits and caveats

- **No theme versioning.** Saving overwrites; no audit log of the
  previous palette. Restoring requires remembering what it was.
- **No theme preview before save.** Live preview card on the admin
  page reflects pending changes; published changes are only visible
  to other tenant routes after save.
- **Contrast check is body-text only on the API.** Three other pairs
  checked by the portal only.
- **Contrast check doesn't cover button hover states.** Designers
  should target a cushion above 4.5:1 to leave room for state
  transitions.
- **No per-page theme overrides.** One row per tenant; a help-center
  page can't have a different palette from the catalog.
- **External `logo_uri` is honour-system.** When `logo_storage_key`
  is NULL the client falls back to `logo_uri`. The server doesn't
  fetch or validate the URL.

## Where to read next

- [Tenant Onboarding](Tenant-Onboarding.md) — first-time playbook
- [Custom Domain + ACME](Custom-Domain-ACME.md) — tenant identity
  extends to the URL
- [API Reference](API-Reference.md#theme) — `/v1/theme/*` endpoints

## Cross-links into the codebase

- `server/migrations/0002_tenant_theme.sql` — base schema
- `server/migrations/0016_footer_branding.sql` — footer toggle
- `server/migrations/0020_tenant_theme_logo.sql` — logo triplet
- `server/migrations/0023_tenant_theme_favicon.sql` — favicon
- `server/src/routes/theme.rs` — API endpoints + validator
- `server/src/logo_sanitize.rs` — SVG deny pass + magic checks
- `web/src/lib/theme.ts` — client Theme type + DEFAULT_THEME
- `web/src/lib/contrast.ts` — WCAG luminance/contrast helpers
- `web/src/hooks.server.ts` — request-time theme resolution
- `web/src/routes/+layout.svelte` — inline `<style>` injection
- `web/src/routes/(authed)/admin/theme/+page.svelte` — admin editor
- `docs/theming.md` — full original theming reference (612 lines)
