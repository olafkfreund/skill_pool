# Theming

## What this covers

The canonical reference for tenant theming in skill-pool: the
`tenant_theme` database schema, the `--sp-*` CSS variables it drives,
how the SvelteKit portal resolves a theme from the request `Host`
header, the default palette, the WCAG AA contrast validation that
gates every save, the "Powered by skill-pool" footer toggle, and the
sanitized logo upload pipeline that ships in migration `0020`. For
larger white-label concerns (custom domains, branded email, regional
asset CDNs) see the cross-links at the foot of this page.

This document covers everything wired up through migration `0020`
(`0002_tenant_theme.sql`, `0016_footer_branding.sql`,
`0020_tenant_theme_logo.sql`). Future-shipping pieces (a tenant
favicon endpoint, a font-family picker) are owned by a sibling spec
and will be appended to this file as separate sections when they
land.

## Schema

The full `tenant_theme` row, as defined by
`server/migrations/0002_tenant_theme.sql` plus the
`footer_branding` column added by
`server/migrations/0016_footer_branding.sql` and the logo-storage
triplet added by `server/migrations/0020_tenant_theme_logo.sql`:

| Column              | Type          | Nullable | Default     | CHECK / Notes |
|---------------------|---------------|----------|-------------|---------------|
| `tenant_id`         | `UUID`        | NO       | —           | Primary key. `REFERENCES tenants(id) ON DELETE CASCADE`. |
| `brand_name`        | `TEXT`        | NO       | —           | API validator caps to 1..=80 chars (`server/src/routes/theme.rs` `validate`). |
| `primary_`          | `TEXT`        | NO       | `'#2563eb'` | Hex colour. CHECK `~ '^#[0-9A-Fa-f]{3,8}$'`. Trailing underscore avoids the SQL reserved word `PRIMARY`. |
| `primary_fg`        | `TEXT`        | NO       | `'#ffffff'` | Hex colour, same CHECK. Foreground colour for buttons / accents painted with `primary_`. |
| `accent`            | `TEXT`        | NO       | `'#0ea5e9'` | Hex colour, same CHECK. |
| `bg`                | `TEXT`        | NO       | `'#ffffff'` | Hex colour, same CHECK. Page background. |
| `fg`                | `TEXT`        | NO       | `'#0f172a'` | Hex colour, same CHECK. Body text. |
| `muted`             | `TEXT`        | NO       | `'#f1f5f9'` | Hex colour, same CHECK. Secondary surface (card backgrounds, tag chips). |
| `muted_fg`          | `TEXT`        | NO       | `'#64748b'` | Hex colour, same CHECK. Secondary text — see "Defaults" for why the API default is `#475569` instead. |
| `border`            | `TEXT`        | NO       | `'#e2e8f0'` | Hex colour, same CHECK. |
| `radius`            | `TEXT`        | NO       | `'0.5rem'`  | CSS length string. Not parsed server-side. |
| `logo_uri`          | `TEXT`        | YES      | NULL        | External logo URL. When `logo_storage_key IS NULL` the client falls back to this. Honour-system: the server does not fetch it. |
| `footer_branding`   | `BOOLEAN`     | NO       | `TRUE`      | When true, "Powered by skill-pool" footer credit is rendered. |
| `logo_storage_key`  | `TEXT`        | YES      | NULL        | Object-store key for the uploaded logo. Set together with the other two `logo_*` columns. |
| `logo_content_type` | `TEXT`        | YES      | NULL        | CHECK `IN ('image/svg+xml', 'image/png', 'image/jpeg', 'image/webp')`. |
| `logo_bytes_size`   | `INTEGER`     | YES      | NULL        | CHECK `0 <= logo_bytes_size <= 262144` (256 KiB). |
| `updated_at`        | `TIMESTAMPTZ` | NO       | `now()`     | Touched by trigger `tenant_theme_touch_updated_at`. |

The three `logo_*` columns share a CHECK
(`tenant_theme_logo_storage_triplet_chk`) that forces all-NULL or all-
NOT-NULL. The GET endpoint depends on this — it always sees the
content-type and the storage key together or it skips serving.

A second CHECK across the eight colour columns
(`tenant_theme_hex_colours`) is defence-in-depth against direct SQL
writes; the JSON validator in `server/src/routes/theme.rs` runs the
same regex on the API path.

## CSS variables

Each theme column drives one CSS custom property. The mapping is set
by `web/src/lib/theme.ts::themeToCss`:

| CSS variable        | Schema column        | Used for |
|---------------------|----------------------|----------|
| `--sp-primary`      | `primary_`           | Primary button background, CTA links. |
| `--sp-primary-fg`   | `primary_fg`         | Text painted on top of `--sp-primary`. |
| `--sp-accent`       | `accent`             | Badge backgrounds, secondary highlights. |
| `--sp-bg`           | `bg`                 | Page background (`<body>`). |
| `--sp-fg`           | `fg`                 | Body text. |
| `--sp-muted`        | `muted`              | Card surfaces, tag chip backgrounds. |
| `--sp-muted-fg`     | `muted_fg`           | Secondary / supporting text. |
| `--sp-border`       | `border`             | Divider lines, input/card outlines. |
| `--sp-radius`       | `radius`             | Border-radius for buttons, cards, inputs. |

(`logo_uri`, `logo_storage_key`, `brand_name`, and `footer_branding`
do not have CSS variables — they're consumed directly by the Svelte
templates.)

Consumers reference the variables exactly like any other custom
property. Tailwind utility classes are wrapped in
`var(--sp-…)` references so the same markup works under any tenant's
palette:

```svelte
<button
  class="rounded-[var(--sp-radius)] px-3 py-1.5 text-sm font-medium"
  style="background: var(--sp-primary); color: var(--sp-primary-fg);"
>
  Install
</button>

<aside class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4">
  <p class="text-sm text-[var(--sp-muted-fg)]">Secondary copy.</p>
</aside>
```

The seed values for the variables also live in CSS (so a request that
fails to resolve a theme still renders). See `web/src/app.css` — the
`:root` block there matches `DEFAULT_THEME` and is overridden per
request by the inline `<style>` block described below.

## Request-time resolution

Theme lookup happens once per HTTP request in
`web/src/hooks.server.ts`. The flow is:

1. `resolveTenant(url, host)` derives the tenant slug:
   - if `?tenant=…` is on the URL, that wins (used by previews and
     local dev);
   - otherwise the leftmost label of the `Host` header is taken
     (`acme.example.com` → `acme`);
   - `www`, `localhost`, an IPv4 literal, or a missing host falls
     back to `SP_DEFAULT_TENANT` env, then to the literal `"default"`.
2. `themeFor(slug)` calls the API server's
   `GET /v1/theme` (no auth — the login page needs branding before
   anyone has logged in; see the comment on `theme::get_theme`).
3. If the API responds, it is converted to the client `Theme` shape
   via `toClientTheme`. If the call throws or the tenant has no row,
   the function returns `{ ...DEFAULT_THEME, brandName: slug }` —
   the slug becomes the visible brand name until an admin saves a
   real one.
4. The resolved `Theme` is stashed on `event.locals.theme` and
   exposed to the page tree by `web/src/routes/+layout.server.ts`.
5. The root layout `web/src/routes/+layout.svelte` calls
   `themeToCss(data.theme)`, wraps the result in `:root { … }`, and
   emits it inside a `<style>` block in `<svelte:head>`. That block
   wins against the seed values in `app.css` because of source
   order; everything painted after the head is rendered uses the
   tenant palette.

Three consequences of this design are worth calling out:

- The theme is computed on **every** request — no caching layer
  sits between the API and the SvelteKit server. The API itself can
  cache its own DB read, but the portal does not.
- Anonymous routes (login, the public catalog) carry the theme
  identically to authed routes; the same hook runs first.
- If the API is unreachable, the portal still renders, just with
  default branding. The failure surfaces on the catalog page rather
  than crashing every request — that is deliberate.

## Defaults

The default palette is duplicated in three places — kept in sync by
hand, and verified by an in-server test
(`server/src/routes/theme.rs::tests::validates_default_theme`):

| Field        | API (`Theme::default_for`) | Web (`DEFAULT_THEME`) | Migration default |
|--------------|----------------------------|-----------------------|-------------------|
| `brand_name` | (tenant slug)              | `'skill-pool'`        | — (NOT NULL, no SQL default) |
| `primary`    | `#2563eb`                  | `#2563eb`             | `'#2563eb'`       |
| `primary_fg` | `#ffffff`                  | `#ffffff`             | `'#ffffff'`       |
| `accent`     | `#0ea5e9`                  | `#0ea5e9`             | `'#0ea5e9'`       |
| `bg`         | `#ffffff`                  | `#ffffff`             | `'#ffffff'`       |
| `fg`         | `#0f172a`                  | `#0f172a`             | `'#0f172a'`       |
| `muted`      | `#f1f5f9`                  | `#f1f5f9`             | `'#f1f5f9'`       |
| `muted_fg`   | `#475569`                  | `#475569`             | `'#64748b'` *     |
| `border`     | `#e2e8f0`                  | `#e2e8f0`             | `'#e2e8f0'`       |
| `radius`     | `0.5rem`                   | `0.5rem`              | `'0.5rem'`        |
| `footer_branding` | `true`                | `true`                | `TRUE`            |

\* `muted_fg` differs between the SQL default (`#64748b`, slate-500)
and the API/web defaults (`#475569`, slate-600). The SQL default
applies only when a `tenant_theme` row is `INSERT`ed without the
`muted_fg` column specified. In practice the API always supplies
every column, so the SQL default is effectively dead code — but it
is also slightly *lower* contrast against `#ffffff` and may fail
the WCAG AA check (see next section) on save. New themes saved via
the admin UI start from the API/web default and therefore pass.

All defaults pass every WCAG AA pair check:

- `fg #0f172a` on `bg #ffffff` = **18.10:1** (AA, AAA-clear)
- `primary_fg #ffffff` on `primary #2563eb` = **5.17:1** (AA-clear)
- `muted_fg #475569` on `muted #f1f5f9` = **6.79:1**
- `muted_fg #475569` on `bg #ffffff` = **7.43:1**

## WCAG AA contrast validation

Every theme save (the `?/save` and `?/reset` form actions on
`web/src/routes/(authed)/admin/theme/+page.server.ts`) runs the
client-side `checkThemeContrast` from `web/src/lib/contrast.ts` and
refuses to call the API if any pair fails. The server then runs its
own body-text-only check inside `validate` in
`server/src/routes/theme.rs` and returns HTTP 400 if it disagrees.

The four pairs:

| Pair | Required ratio | Why |
|------|----------------|-----|
| `fg` / `bg`                                  | **4.5:1** (AA, body) | Page-level body text. |
| `primaryFg` / `primary`                      | **3.0:1** (AA, UI)   | Button text on the CTA colour. WCAG treats button labels as UI components, where the threshold is 3:1. |
| `mutedFg` / `muted`                          | **4.5:1** (AA, body) | Secondary text rendered on top of a card / chip background. |
| `mutedFg` / `bg`                             | **4.5:1** (AA, body) | The same secondary text rendered directly on the page background. |

What happens on save when a pair fails:

1. The admin theme page (`web/src/routes/(authed)/admin/theme/+page.svelte`)
   computes the badges live as the colour pickers change, and disables
   the Save button while any pair is failing
   (`const blocked = $derived(liveFailures.length > 0)`). The
   `disabled` attribute on the submit button blocks the request from
   leaving the browser.
2. If a request *does* reach the server (e.g. an API client bypassing
   the UI), the form action re-runs `checkThemeContrast` and returns
   `fail(422, { contrastFailures, draft })`. The page renders a red
   "Save blocked" banner listing the failing pairs with their actual
   ratios.
3. The server-side validator
   (`server/src/routes/theme.rs::validate`) also runs the
   fg / bg pair check on `PUT /v1/theme` and returns
   400 + `"body text contrast (fg vs bg) is X:Y; WCAG AA requires
   4.5:1"`. It does **not** run the primary/muted pair checks today —
   only the body-text pair. The portal-side check is the only place
   the full four-pair audit happens, so direct API consumers (curl,
   CI, integrations) can save themes that fail the UI checks. This is
   a known asymmetry; see "Limits and caveats".

The client mirror of the luminance + contrast math is in
`web/src/lib/contrast.ts`. It uses the same WCAG relative-luminance
formula as the Rust implementation (sRGB linearisation with the
0.03928 threshold; weighted Y = 0.2126·R + 0.7152·G + 0.0722·B), and
the unit test `contrast_of_black_on_white_is_21` in
`server/src/routes/theme.rs` pins the two implementations together at
the only ratio either of them could be confidently wrong about.

The badges rendered next to each pair use four levels:

| Level | Threshold | Visual style |
|-------|-----------|--------------|
| `AAA`       | ≥ 7.0:1 | emerald |
| `AA`        | ≥ 4.5:1 | sky     |
| `AA-large`  | ≥ 3.0:1 | amber — passes only for large text or UI components |
| `fail`      | < 3.0:1 | red     |

See `wcagBadge` in `web/src/lib/contrast.ts` and `badgeClass` in the
admin theme `+page.svelte`.

## Footer branding toggle

The `footer_branding` boolean controls whether the site footer
renders a "Powered by skill-pool" credit. Default is `TRUE` — every
new tenant ships with the credit visible.

Who can flip it: any caller holding the `tenant:admin` scope (the
same scope that gates every other theme write — see
`require_scope("tenant:admin")` at the top of `put_theme`,
`post_logo`, and `delete_logo`). The admin theme page exposes it as
a plain checkbox under the colour pickers and submits it with the
rest of the palette in one PUT.

Tier gating: **none today**. Every tier can turn the footer off; the
boolean is a free knob. A future stretch is to lock it ON for the
Free tier and let Team/Enterprise tenants disable it. That gating
would go into `validate` in `server/src/routes/theme.rs` and reject
`footer_branding=false` based on the tenant's tier. It is not in
the schema today and there is no tier column to key off of yet.

## Logo upload + sanitization

Four endpoints, all routed in `server/src/routes/mod.rs`:

| Method   | Path             | Auth                      | Body |
|----------|------------------|---------------------------|------|
| `GET`    | `/v1/theme`      | None (public)             | — |
| `PUT`    | `/v1/theme`      | `tenant:admin` scope      | JSON `Theme` |
| `GET`    | `/v1/theme/logo` | None (public)             | — |
| `POST`   | `/v1/theme/logo` | `tenant:admin` scope      | `multipart/form-data` with one `file` part |
| `DELETE` | `/v1/theme/logo` | `tenant:admin` scope      | — |

**Accepted content types** on upload (`server/src/logo_sanitize.rs`,
mirrored in the DB CHECK on `logo_content_type`):

- `image/svg+xml` (and `image/svg`, normalised)
- `image/png`
- `image/jpeg` (and `image/jpg`, normalised)
- `image/webp`

Anything else returns `400 unsupported content type "...". The
multipart part's own `Content-Type` header is what the server reads —
not anything from `Authorization` or another client-controlled state.

**Size cap** is 256 KiB (`MAX_LOGO_BYTES` in `logo_sanitize.rs`),
enforced at three layers:

1. The DB CHECK constraint `tenant_theme_logo_bytes_size_chk`
   (`0 <= size <= 262144`) — defence in depth against direct SQL
   writes.
2. The multipart handler in `post_logo` returns 400 before reaching
   the sanitizer if `raw.len() > MAX_LOGO_BYTES`.
3. The Axum router body-limit middleware (operator-configured) wraps
   the request and refuses oversized bodies before any handler
   runs.

**SVG reject list.** The hand-rolled deny pass in
`logo_sanitize::sanitize_svg` lowers the bytes once and refuses
uploads containing any of:

- Element openers — `<script`, `</script`, `<foreignobject`,
  `<iframe`, `<embed`, `<object`, `<base`, `<link`, `<meta`.
- Dangerous URI schemes — `javascript:`, `vbscript:`,
  `data:text/html`, `data:application/javascript`,
  `data:text/javascript`.
- CSS escapes — `expression(`, `@import`, `behavior:`.
- Inline event handlers — any `on<letters>=` attribute (caught by
  the hand-rolled state machine `has_event_handler_attr`; whitespace
  before `=` is tolerated as a bypass shape and still rejected).
- Off-origin `href` / `xlink:href` — values must be empty or a
  same-document fragment (`#…`). External URLs, `data:` URIs, and
  filesystem paths are rejected (`has_off_origin_href`).

After the deny pass an SVG is parsed by `quick-xml` and re-emitted
without comments or processing instructions. Malformed XML is
rejected outright (`SanitizeError::MalformedSvg`). CDATA blocks are
re-escaped as text — the deny pass already caught `<script` inside
them, but the re-escape closes the loophole where a downstream
renderer treats CDATA as raw markup.

Raster formats are **not** re-encoded — see the head comment in
`server/src/logo_sanitize.rs` for the threat-model reasoning.
Instead, the server checks the magic bytes against the claimed
content-type:

- PNG: `89 50 4E 47 0D 0A 1A 0A`
- JPEG: `FF D8 FF`
- WEBP: `RIFF` + 4 bytes + `WEBP`

Any mismatch returns 400 `magic bytes do not match content type
"..."`.

**Storage key shape.**
`server/src/storage.rs::Storage::logo_key` returns
`{tenant_id}/theme/logo.{ext}` where `ext` is one of `svg`, `png`,
`jpg`, `webp` (canonical, from `LogoKind::extension`). All four
tenants' logos collide on the `/theme/logo.*` prefix inside their
own UUID-scoped namespace, which means a re-upload of the same
format overwrites the old object in place. A format change (e.g. SVG
→ PNG) writes a new key — `post_logo` issues a best-effort delete
of the previous key when the new extension differs, so storage
stays tidy. The delete is intentionally best-effort: failing to
clean up an orphan never blocks the upload.

**Cache headers on GET.** `GET /v1/theme/logo` returns
`Cache-Control: public, max-age=300` (five minutes). The comment in
`server/src/routes/theme.rs::get_logo` explains the trade-off — long
enough to dodge login-page load, short enough that a logo replace
shows up across the org within minutes. See
[`docs/enterprise/asset-cdn.md`](./enterprise/asset-cdn.md) for the
downstream CDN implications.

**Audit trail.** Every logo upload and delete emits an audit event
via `audit::record_best_effort` (`theme.logo.upload` /
`theme.logo.delete`) carrying the content-type and size. Theme
saves emit `theme.update` with the full payload. See
`docs/api.md` for the audit-event schema.

## Brand name and length limits

`brand_name` is bounded to **1..=80 characters** by
`validate` in `server/src/routes/theme.rs`:

```rust
if t.brand_name.is_empty() || t.brand_name.len() > 80 {
    return Err(AppError::BadRequest("brand_name must be 1..=80 characters".into()));
}
```

The schema does not enforce a length cap directly (the column is
plain `TEXT NOT NULL`), so direct SQL writes can exceed 80 — every
API-mediated write respects the cap. The web admin's free-text
input has no `maxlength` attribute; the server is the authority.

The brand name appears in:

- `<title>` (rendered as `{brandName} · skill-pool` by the root
  layout in `web/src/routes/+layout.svelte`).
- A watermark on the live preview card in the admin theme page.
- Anywhere else a Svelte template references `data.theme.brandName`.

## Limits and caveats

Known gaps in the v1 theming surface — none of these are critical
but operators should know about them:

- **No theme versioning.** Saving a theme overwrites the previous
  row; there is no audit log of the previous palette. The
  `theme.update` audit event carries the full new theme but not the
  diff against the old one. Restoring the previous theme requires
  remembering what it was.
- **No theme preview before save.** The admin page renders a *live*
  preview card as the colour pickers change, but the change is not
  visible on other tenant routes until the save lands. There is no
  "preview this theme as a different user" toggle.
- **Contrast check is body-text only on the API.** The server-side
  validator (`validate` in `theme.rs`) only enforces the fg/bg pair.
  The other three pairs (`primaryFg`/`primary`, `mutedFg`/`muted`,
  `mutedFg`/`bg`) are checked by `checkThemeContrast` in the portal
  only. A direct API caller (curl, CI) can save a theme that fails
  the UI checks. The asymmetry is a known follow-up.
- **Contrast check does not cover button hover states.** Hovered or
  active buttons may end up with a darker/lighter `primary` than
  the colour the validator was given. Designers should target a
  comfortable cushion above 4.5:1 (or 3:1 for UI) to leave room for
  state transitions.
- **No per-page theme overrides.** One row per tenant. A help-center
  page cannot have a different palette from the catalog. Adding
  this would require a route-keyed lookup; not planned.
- **`muted_fg` SQL default is below WCAG AA in isolation.** The
  migration default `#64748b` against `#ffffff` is 4.36:1 — under
  the 4.5:1 threshold. Since the API always overrides this on save
  (with `#475569`) the cell is effectively unreachable, but a
  future schema change that inserts a row with column defaults only
  would land an out-of-policy palette. Fix is to bump the SQL
  default to match `Theme::default_for`.
- **External `logo_uri` is honour-system.** When `logo_storage_key`
  is NULL the client falls back to `logo_uri`. The server does not
  fetch or validate the URL — a tenant can point it anywhere and
  the browser will render it. Pair this with the CSP guidance in
  [`docs/enterprise/asset-cdn.md`](./enterprise/asset-cdn.md) when
  hardening a production deploy.
- **Cached storage backend means logo writes don't propagate on a
  residency change.** When a tenant flips their `storage_uri`, the
  per-process backend cache means subsequent logo uploads still
  land in the old bucket until restart. See the cache discussion in
  [`docs/enterprise/data-residency.md`](./enterprise/data-residency.md).

## Related files and cross-links

- `server/migrations/0002_tenant_theme.sql` — base schema
- `server/migrations/0016_footer_branding.sql` — footer toggle
- `server/migrations/0020_tenant_theme_logo.sql` — logo triplet
- `server/src/routes/theme.rs` — API endpoints + validator
- `server/src/logo_sanitize.rs` — SVG deny pass + magic checks
- `web/src/lib/theme.ts` — client `Theme` type + `DEFAULT_THEME`
- `web/src/lib/contrast.ts` — WCAG luminance/contrast helpers
- `web/src/hooks.server.ts` — request-time theme resolution
- `web/src/routes/+layout.svelte` — inline `<style>` injection
- `web/src/routes/(authed)/admin/theme/+page.svelte` — admin editor
- `web/src/routes/(authed)/admin/theme/+page.server.ts` — form actions
- [`docs/enterprise/asset-cdn.md`](./enterprise/asset-cdn.md) —
  how themed assets (logos today, favicons + OG images later) can
  be fronted by a CDN; CSP implications of doing so.
- [`docs/enterprise/branded-emails.md`](./enterprise/branded-emails.md)
  — the same `brand_name` is reused in transactional email headers
  when the email-branding row is present.
- [`docs/enterprise/data-residency.md`](./enterprise/data-residency.md)
  — when `tenants.storage_uri` is set, the logo blob also lives in
  the tenant's regional bucket; CDN placement should follow.
- [`docs/enterprise/custom-domains.md`](./enterprise/custom-domains.md)
  — `resolveTenant` in the SvelteKit hook reads the slug from the
  custom domain's leftmost label, so the right tenant theme is
  served the moment the custom domain is verified.

## 11. Favicon

Tenant admins may upload an explicit favicon at `POST /v1/theme/favicon`.
When no favicon has been uploaded but a logo has, `GET /v1/theme/favicon`
transparently serves the logo bytes so every browser tab still shows the
tenant's brand mark.

### Endpoints

| Method | Path                 | Auth           | Notes                                      |
|--------|----------------------|----------------|--------------------------------------------|
| POST   | `/v1/theme/favicon`  | `tenant:admin` | Multipart; single `file` field.            |
| DELETE | `/v1/theme/favicon`  | `tenant:admin` | 204 on success.                            |
| GET    | `/v1/theme/favicon`  | public         | Falls back to logo bytes when unset.       |

### Content-types

The favicon endpoint accepts the same four formats as the logo endpoint plus
`image/x-icon` for classic Windows ICO files:

- `image/svg+xml`
- `image/png`
- `image/jpeg`
- `image/webp`
- `image/x-icon` (alias: `image/vnd.microsoft.icon`)

SVG payloads run through the same hardened sanitizer the logo uses —
`<script>`, event-handler attributes, off-origin `xlink:href`, CSS escapes
(`expression(...)`, `@import`), and friends are rejected. Raster formats are
magic-checked but not re-encoded; ICO files are validated to have the proper
ICONDIR header.

### Size cap

64 KiB. Smaller than the logo's 256 KiB cap because favicons should be tiny
and a tight ceiling makes accidental "upload-the-hero-image" mistakes fail
loudly. Both the route layer and the DB CHECK constraint in migration `0023`
enforce it.

### Fallback semantics

| Favicon set | Logo set | `GET /v1/theme/favicon` |
|-------------|----------|--------------------------|
| yes          | (either) | Favicon bytes            |
| no          | yes       | Logo bytes (same `Cache-Control: public, max-age=300`) |
| no          | no       | `404`                    |

The fallback means a tenant can upload **just** a logo and still have a
sensible favicon — most browsers happily scale an SVG or PNG to whatever
size the chrome needs.

### Cache headers

Responses carry `Cache-Control: public, max-age=300` — the same five-minute
window the logo uses. Long enough to dodge load on the login page, short
enough that a favicon replace is visible across the org within minutes.

### Storage key

`{tenant_id}/theme/favicon.{ext}` — sibling to the logo key. Data-residency
overrides flow through `state.storage_for(&tenant)`, so a tenant pinned to a
different storage bucket sees its favicon land in the right place.

### Follow-up (not implemented)

Server-side rasterization of the uploaded asset into discrete PNG sizes
(`16x16`, `32x32`, `180x180` for `apple-touch-icon`, `192x192` and `512x512`
for web app manifests) is **not** implemented yet. It would require pulling
in a decode/encode stack (`resvg` for SVG → PNG, `image` for resampling)
and the CPU cost is hard to justify on every upload.

For now, operators who want discrete sizes have two options:

1. Upload an SVG favicon — every modern browser will render it at any size
   the chrome asks for.
2. Upload individual sized PNGs and reference them with multiple
   `<link rel="icon" sizes="32x32">` tags in their own portal shell.

Tracked under the "favicon auto-rasterization" follow-up box on issue #9.

## 12. Font picker

The portal ships a curated Google-Fonts allowlist so a tenant admin can pick
a brand-aligned typeface without writing CSS. The list lives in
`server/src/routes/theme.rs::ALLOWED_FONTS` and is mirrored by the web
load function so the picker UI can populate even when the API can't be
reached.

### Allowlist

| Family                  | Style       | Why it's on the list                          |
|-------------------------|-------------|-----------------------------------------------|
| `system`                | (varies)    | OS-native stack — zero network cost; default. |
| `Inter`                 | Sans        | De-facto standard for dashboard UIs.          |
| `IBM Plex Sans`         | Sans        | Excellent for data-dense tables.              |
| `JetBrains Mono`        | Monospace   | Code-heavy UIs.                               |
| `Source Sans 3`         | Sans        | Neutral, well-balanced.                       |
| `Source Serif 4`        | Serif       | Companion serif for editorial layouts.        |
| `Merriweather`          | Serif       | Proven body-text serif, optimized for screens.|
| `Roboto`                | Sans        | Near-universal recognition.                   |
| `Fira Sans`             | Sans        | Friendly humanist tone.                       |
| `Atkinson Hyperlegible` | Sans        | Accessibility-first; great low-vision support.|
| `Work Sans`             | Sans        | Modern grotesque, optimized for screen.       |
| `Lora`                  | Serif       | Contemporary serif for long-form content.     |

Every entry is licenced under the SIL Open Font Licence (OFL) or Apache 2.0,
which means both web embedding **and** self-hosting are permitted.

### How the portal loads it

When the tenant picks a non-`system` family, the portal injects a
`<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=...">`
into the document head and sets `--sp-font-family` on `:root`. The chosen
face renders against the next paint, no rebuild required.

Picking `system` skips the `<link>` injection entirely and resolves to the
OS-native font stack (`system-ui, -apple-system, BlinkMacSystemFont, ...`).

### API

- `GET /v1/theme/fonts` — public. Returns `{ "allowed": [...12 names...] }`.
  Useful for any client (the portal, third-party admin consoles) that wants
  to render a picker without hard-coding the list.
- `PUT /v1/theme` — accepts `font_family` (optional). Values not in the
  allowlist are rejected with `400 BadRequest` and an error message naming
  the rejected value.

### CSS variable

`--sp-font-family` carries the resolved stack (quoted family name plus a
sensible fallback tail). The root `+layout.svelte` applies it to `body` so
every page inherits the chosen face without per-component opt-in:

```css
body { font-family: var(--sp-font-family, system-ui); }
```

### Self-hosting

If you want the font self-hosted (no third-party `<link>` to Google), do this:

1. Pick `system` in the picker so the portal does **not** inject the Google
   Fonts stylesheet.
2. Add your own `@font-face` declarations in a custom-CSS layer. Once the
   custom-CSS upload feature ships (separate item on #9), you'll be able to
   drop `@font-face` rules straight into the tenant theme; for now the
   operator deploying skill-pool can splice them into the portal's
   `app.css`.

Because every font in the allowlist is OFL or Apache 2.0, downloading the
font files and shipping them under `static/fonts/` is permitted.

### Why an allowlist?

Letting tenants pass arbitrary CSS through the `font-family` column would
open a small but real injection vector (CSS unicode-escape, fallback chain
abuse). A 12-entry server-side allowlist keeps the column to known-good
values and keeps the picker UI honest about what's supported.
