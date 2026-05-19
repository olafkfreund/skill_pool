# Custom CSS overlay

## Scope

This document covers the **per-tenant custom CSS overlay** — a small
stylesheet that an Enterprise tenant can upload to layer additional
brand polish on top of the curated `--sp-*` theme variables that drive
every page of the portal. The feature is implemented by:

- `server/migrations/0025_tenant_custom_css.sql` — adds
  `custom_css_storage_key` and `custom_css_bytes_size` columns on
  `tenant_theme`.
- `server/src/css_sanitize.rs` — the deny-rule sanitizer.
- `server/src/routes/theme.rs::{post_custom_css, delete_custom_css, get_custom_css}` —
  the HTTP surface.
- `web/src/routes/theme/custom.css/+server.ts` — the SvelteKit proxy.

For the upstream theme model (colour variables, fonts, logo, favicon)
see [`../theming.md`](../theming.md). For how to put the overlay
behind a CDN see [`./asset-cdn.md`](./asset-cdn.md) — the response is
already cacheable and the path is stable.

## Story

Enterprise tenants ship with a defined corporate identity that
extends past colour and logo: a hover treatment on links, a
specific border-radius on hero cards, a brand-shaped focus ring.
The `tenant_theme` row covers the headline variables, but a tenant
that wants the last 5% of polish needs a way to override individual
class selectors without forking the portal.

The custom-CSS overlay is that surface. The server stores a small
(≤ 32 KiB) CSS file per tenant, and the portal serves it at
`/v1/theme/custom.css` with cache + CSP headers. The root layout
injects a `<link rel="stylesheet">` for the file *only* when the
tenant has one set, so non-customised tenants pay zero overhead.

## What's allowed

CSS that targets the portal's own DOM:

- Class overrides on `.sp-*` classes (`.sp-hero`, `.sp-card-header`,
  `.sp-tag-pill`, etc.). The class names that ship in the portal are
  stable; we treat them as a public surface.
- Modifications of CSS custom properties (`:root { --sp-radius: 0.25rem; }`).
  Variables are the recommended override path for everything that
  has one — they cascade through every component for free.
- Descendant / combinator selectors (`.sp-card .sp-tag-pill`).
- Pseudo-classes and pseudo-elements (`:hover`, `:focus-visible`,
  `::before`, `::after`).
- Inline `data:image/...` and `data:font/...` URIs in `url()` —
  pre-encoded gradients, brand-shape SVGs, and self-hosted fonts
  served as base64.
- Same-document fragment refs in `url()` (`url(#brand-gradient)`) —
  references to `<defs>` blocks the portal renders elsewhere.
- `@font-face` declarations that point at `data:` URIs.

## What's rejected

The sanitizer (`server/src/css_sanitize.rs`) is a deny-first byte
scan. The bytes are first lower-cased and stripped of `/* ... */`
comments (see "Why strip then rescan" below) and then scanned for
each of:

| Rule                                        | Error variant            | Why                                                              |
|---------------------------------------------|--------------------------|------------------------------------------------------------------|
| `@import`                                   | `ImportRule`             | Pulls in an external stylesheet at parse time.                   |
| `expression(`                               | `Expression`             | Legacy IE: executes JS in CSS context.                           |
| `behavior:`                                 | `BehaviorBinding`        | Legacy IE: binds CSS classes to HTC files.                       |
| `javascript:` URI                           | `JavascriptUri`          | Executes JS in any context that fetches the URL.                 |
| `vbscript:` URI                             | `VbscriptUri`            | Same shape; legacy IE.                                           |
| `data:text/html`                            | `DataHtml`               | HTML payload smuggled as a "stylesheet" reference.               |
| `data:text/javascript`                      | `DataJavascript`         | JS payload, ditto.                                               |
| `data:application/javascript`               | `DataAppJavascript`      | JS payload, ditto.                                               |
| `url()` pointing off-site                   | `ExternalUrl`            | Sanitizes against rogue tracking pixels and exfiltration.        |
| `<script`, `<iframe`, `<object`, `<embed`,  | `HtmlTag`                | Defends against CSS-in-HTML injection contexts.                  |
| `<link`, `<meta`, `<base`                   |                          |                                                                  |
| `</style>`                                  | `StyleClose`             | Classic "break out of `<style>`" shape; always hostile.          |
| Bytes that aren't valid UTF-8               | `Utf8`                   | DB column is TEXT; reject up front.                              |
| Empty body                                  | `Empty`                  | Use DELETE to clear.                                             |
| > 32 KiB                                    | `TooLarge`               | Enforced in three places (CHECK, route, body limit).             |

### Why strip-then-rescan is mandatory

A classic bypass pattern is to hide a forbidden token behind a CSS
comment so that a naive "string contains" scan misses it:

    /* harmless */@import url(evil);

The comment-stripping pass in `strip_comments_and_lower` removes
the `/* ... */` block before the deny-rule scan runs, so the
contiguous `@import` becomes visible to the scanner. We then
persist the **original** bytes (comments intact) so the tenant
sees their authored content back when they edit — only the
scan haystack is mutated. The integration test
`rejects_comment_hidden_import` enforces this invariant on every
build.

## CSP discussion

The `GET /v1/theme/custom.css` response sets three headers:

    Content-Type: text/css; charset=utf-8
    Cache-Control: public, max-age=300
    Content-Security-Policy: style-src 'self'
    X-Content-Type-Options: nosniff

The `Content-Security-Policy: style-src 'self'` line is defence in
depth under the sanitizer. Even if a sanitizer bypass slipped past
and the persisted bytes contained an `@import url(https://evil.com/x.css)`,
the browser's enforcement of the response-level CSP would refuse to
fetch the referenced sheet. This guarantee is **independent** of the
parent document's CSP — when a resource sets its own CSP, browsers
treat the resource itself as the policy context for nested fetches.

The parent document (the portal HTML) must still allow
`style-src 'self' 'unsafe-inline'` because the root layout emits an
inline `<style>` block for the theme variables. We have not yet
shipped nonce-based CSP for the inline block — that's a follow-up
documented in the [theming.md](../theming.md) TODOs. Any future
CSP-tightening pass must keep `'unsafe-inline'` until that work
lands; removing it would break the theme system, not just the
custom-CSS overlay.

`X-Content-Type-Options: nosniff` closes the legacy IE / Edge
escape hatch where a response served as `text/css` could be
re-interpreted as `text/html` by content sniffing.

## Cache behaviour

`Cache-Control: public, max-age=300` — five minutes. Reasoning:

- A custom-CSS overlay is rendered into the cascade on every
  authed page load *and* on the login page. Short cache lifetimes
  multiply request volume; long ones delay propagation after an
  upload.
- Five minutes matches the logo and favicon endpoints. An admin
  hot-fixing a brand glitch sees the new rules within five minutes
  on every browser; refresh-skipping users wait at most one extra
  navigation cycle.
- The route key (`{tenant_uuid}/theme/custom.css`) is stable — a
  CDN in front of the endpoint can cache aggressively without
  worrying about cache poisoning across tenants because the API
  resolves the tenant from `X-Skill-Pool-Tenant` (or the host
  binding in dedicated mode) before serving.

If you need an instant invalidation (e.g. for a launch coordinated
to the minute), upload an empty-deletion + the new file with a
purposeful sub-cache pause — or front the endpoint with a CDN that
exposes a manual purge API (see `asset-cdn.md`).

## Setup recipes

### Via the web UI

1. Navigate to `Settings → Theme` as a tenant admin.
2. Scroll to **Custom CSS overlay**.
3. Paste your CSS into the textarea. The byte counter under the
   textarea updates as you type; submission is blocked at 32 KiB.
4. Click **Save custom CSS**. The page reloads and the overlay
   takes effect on the next navigation (you may need to soft-reload
   to see the previously-cached stylesheet expire).
5. To remove, click **Remove custom CSS**.

### Via curl

Upload — `overlay.css` is your local file:

```sh
curl -sS \
  -H "X-Skill-Pool-Tenant: acme" \
  -H "Authorization: Bearer $SKILL_POOL_ADMIN_TOKEN" \
  -F "file=@overlay.css;type=text/css" \
  "$SKILL_POOL_API_BASE/v1/theme/custom-css"
```

Verify the GET surface:

```sh
curl -sS -i \
  -H "X-Skill-Pool-Tenant: acme" \
  "$SKILL_POOL_API_BASE/v1/theme/custom.css" | head
```

You should see `Content-Security-Policy: style-src 'self'`,
`Content-Type: text/css; charset=utf-8`, and `Cache-Control:
public, max-age=300` in the response.

Clear:

```sh
curl -sS -X DELETE \
  -H "X-Skill-Pool-Tenant: acme" \
  -H "Authorization: Bearer $SKILL_POOL_ADMIN_TOKEN" \
  "$SKILL_POOL_API_BASE/v1/theme/custom-css"
```

### Recommended overlay shape

```css
/* Acme Corp brand overlay */
:root {
  --sp-primary: #d40000;     /* prefer setting these via the theme editor */
  --sp-radius: 0.25rem;      /* falls back through the editor's value */
}

/* Hero card polish */
.sp-card-header {
  letter-spacing: -0.01em;
}

/* Brand-shaped focus ring (not exposed as a variable) */
:focus-visible {
  outline: 3px solid var(--sp-primary);
  outline-offset: 2px;
}
```

Anything that *is* exposed as a `--sp-*` variable should go through
the theme editor — the editor enforces WCAG AA contrast and is
visible to operators who don't touch the overlay. Keep the overlay
for shapes that don't have a variable.

## Limitations

- **No preview before save.** The editor textarea is a plain
  `<textarea>`; the new rules apply after the page reload. This is
  intentional — a live preview would have to re-render the entire
  portal in an isolated iframe and is out of scope. If you need a
  preview, save into a staging tenant.
- **No syntax validation.** A malformed rule (`color: notacolor;`)
  is accepted by the sanitizer and silently dropped by the browser.
  Operators get to break their own UI; we don't run a CSS parser
  before persisting because pulling one in for this surface is
  disproportionate.
- **No per-page overrides.** The overlay is global. Per-page
  overrides would require shipping page-class hooks into the
  layout templates — track that as a separate feature.
- **No subsetting.** We persist and serve the bytes verbatim. If
  your overlay contains a 30 KiB unused selector, every visitor
  pays for it. Minify locally before uploading.
- **No version history.** Each upload overwrites the previous one
  at the same storage key. If you need a rollback workflow, keep
  the source CSS in the tenant's own repo and re-upload on demand.

## Auditing

Every upload and delete writes an audit event:

| `action`                  | `target_kind` | `metadata`            |
|---------------------------|---------------|-----------------------|
| `theme.custom_css.upload` | `theme`       | `{ size_bytes: ... }` |
| `theme.custom_css.delete` | `theme`       | `null`                |

The events fan out through the SIEM export pipeline (see
[`./audit-siem.md`](./audit-siem.md) if your tenant has it
configured). The byte size — not the content — is in the metadata
so an operator can trace "the overlay got 4 KiB bigger on Tuesday"
without retaining the CSS contents in the audit stream.
