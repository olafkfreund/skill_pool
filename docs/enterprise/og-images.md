# Open Graph (social-card) images

> Enterprise tier. Closes one of the boxes on issue [#9](https://github.com/calitii/skill-pool/issues/9).

When somebody pastes a link to one of your skills into Slack, Discord,
Microsoft Teams, Twitter/X, or LinkedIn, the platform fetches the page
and looks at the `<meta property="og:image">` tag to decide what to
preview. skill-pool generates that image automatically, per-tenant and
per-skill, branded with your colours and logo.

No setup required. As soon as a tenant has a theme and a published
skill, every shared link gets a card.

---

## What you get

A 1200×630 social card containing:

- Your **brand colours** (background, primary border, foreground).
- Your **logo** in the top-left (or a coloured initial if no logo is
  uploaded).
- Your **brand name** next to the logo.
- The **skill name** as a large headline.
- The **description**, wrapped to ~70 characters across up to four
  lines, ellipsised if longer.
- A **version pill** in the bottom-right: `v1.2.3`, primary-coloured.
- A **kind label** in the bottom-left: `kind: skill | agent | command`.

The skill detail page (`/skills/<slug>`) renders the relevant
`<svelte:head>` block:

```html
<meta property="og:title"       content="axum-handler · Acme Corp" />
<meta property="og:description" content="A practical recipe for clean axum route handlers." />
<meta property="og:image"       content="https://acme.skill-pool.example.com/v1/og?slug=axum-handler" />
<meta property="og:url"         content="https://acme.skill-pool.example.com/skills/axum-handler" />
<meta name="twitter:card"       content="summary_large_image" />
```

---

## Platform support — be honest about the format

We ship **SVG**, not PNG. This is a deliberate trade-off: SVG renders
crisply on any DPI, generates in microseconds, and adds zero new
build-time dependencies. The downside is that not every social platform
treats SVG as a first-class `og:image`.

| Platform              | Behaviour with SVG                       |
| --------------------- | ---------------------------------------- |
| Slack                 | Full preview, scales to message width.   |
| Discord               | Full preview.                            |
| Microsoft Teams       | Full preview (Teams renders any image).  |
| iMessage / Messages   | Full preview.                            |
| Twitter / X           | **Broken thumbnail** — requires PNG/JPG. |
| LinkedIn              | **Broken thumbnail** — requires PNG/JPG. |
| Facebook              | **Broken thumbnail** — requires PNG/JPG. |
| Google search snippet | Indexed but not always shown.            |

For most engineering teams the Slack-and-Discord case dominates the
share volume by an enormous margin, so this trade-off is the right one
out of the box. If your team needs Twitter / LinkedIn previews, see
"Future work" below.

---

## Cache behaviour

The endpoint returns:

```
Cache-Control: public, max-age=86400
ETag: "og-<16-hex-chars>"
```

That's a one-day shared cache. The ETag is composed from:

- The tenant ID.
- The skill slug.
- The catalog kind (`skill` / `agent` / `command`).
- The skill version (re-publishing a skill changes the version, which
  changes the ETag).
- The tenant theme's `updated_at` (changing colours, brand name, or
  logo bumps the ETag).

A `GET` with `If-None-Match: <etag>` returns `304 Not Modified` so
intermediate caches (your CDN, the social-platform's own scraper
cache) revalidate cheaply.

### The social-platform cache caveat

The big asterisk: **the social platforms cache aggressively, and they
do not honour your `Cache-Control` for inline images.** Empirically:

- Slack caches `og:image` for ~24 hours after first crawl.
- Discord caches indefinitely until the message is re-edited.
- Twitter/X re-scrapes when the URL is shared again (which on a busy
  account may be never).
- LinkedIn caches for 7+ days.

If you rebrand (change theme colours, swap logo, rename the company) the
freshly-shared links will pick up the new card immediately, **but every
already-shared link in already-posted Slack messages will keep
showing the old card** until the platform's cache expires. This is
platform behaviour, not something skill-pool can override. There is no
"flush all social caches" button.

The usual remedies are:

- Re-share the link to force a re-scrape.
- Use Facebook's [Sharing Debugger](https://developers.facebook.com/tools/debug/) or [X's Card Validator](https://cards-dev.twitter.com/validator) to manually trigger a re-fetch.
- Wait it out.

---

## The endpoint

```
GET /v1/og?slug=<slug>&kind=<skill|agent|command>
```

Tenant-resolved by `Host` header or `X-Skill-Pool-Tenant`. No auth —
social crawlers don't carry credentials.

| Param  | Required | Default | Notes                                    |
| ------ | -------- | ------- | ---------------------------------------- |
| `slug` | yes      | —       | Skill slug. Empty / missing → `400`.     |
| `kind` | no       | `skill` | `skill`, `agent`, or `command`. Else 400.|

Responses:

| Status | Reason                                                |
| ------ | ----------------------------------------------------- |
| `200`  | SVG body, `Content-Type: image/svg+xml`, has ETag.    |
| `304`  | `If-None-Match` matched.                              |
| `400`  | Missing `slug`, empty `slug`, or unknown `kind`.      |
| `404`  | Slug not found (no published row in this kind).       |

Choice point: a 404 (rather than serving a generic fallback image)
means the page-level `og:image` simply disappears from the preview if
the skill is deleted. We considered serving a "skill not found" card
but rejected it: social-platform crawlers cache 200s aggressively, and
a stale "not found" card stuck in the cache is worse than no card.

---

## Limitations

- **No per-skill image override.** Operators can't pin a hand-crafted
  image to one particular skill — every card is generated from the
  same template. Roadmap item if there's demand.
- **Locale-default font.** The SVG declares `font-family:
  system-ui, sans-serif`. The renderer that actually rasterises the
  SVG (the social platform's, not ours) picks whatever font that
  resolves to in its environment. Non-Latin scripts render character-
  by-character.
- **Raster-only platforms (Twitter, LinkedIn, Facebook) won't
  preview.** SVG → PNG conversion would mean adding `resvg` +
  `tiny-skia` to the server build (about 4 extra crates and a
  measurable cold-build penalty). We didn't take that hit at first
  ship; see "Future work".
- **Logo embedding is best-effort.** SVG logos and raster logos
  (PNG/JPEG/WebP) are both embedded as data URIs inside the SVG. If
  the underlying storage fetch fails for any reason, the renderer
  falls back to the brand initial in a coloured circle rather than
  failing the whole request.

---

## Future work

If the Twitter/LinkedIn gap becomes a real problem for your team, the
fix is a second renderer path that rasterises the same SVG to PNG using
`resvg` + `tiny-skia` (both pure-Rust, no system deps beyond the Rust
toolchain). The handler would content-negotiate on `Accept`, or accept
a `&format=png` query param.

The work is roughly:

- Add the two crates to `server/Cargo.toml`.
- A `render_png(svg)` helper using `resvg::Tree::from_str` and
  `tiny_skia::Pixmap`.
- Branch in `og_image()` based on the requested format.
- Add a PNG path to the integration test.

We left this for a follow-up because the dependency cost is real (about
30 seconds added to a from-scratch `cargo build --release`) and
Slack/Discord previews — the majority case for internal teams — work
perfectly with SVG alone.
