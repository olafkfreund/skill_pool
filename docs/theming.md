# Theming (issue #9)

This document describes the per-tenant theming surface: brand colours, logo,
favicon, footer credit, and font picker.

<!-- Sections 1-10 (palette, contrast rules, logo upload, footer branding,
     content-type rules, limits and caveats, etc.) are authored by the
     sibling theming-docs spec. Sections 11 and 12 below are owned by the
     favicon + font-picker work. -->

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
| ✅          | (either) | Favicon bytes            |
| ❌          | ✅       | Logo bytes (same `Cache-Control: public, max-age=300`) |
| ❌          | ❌       | `404`                    |

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
