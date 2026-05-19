# Asset CDN

## Scope

This document covers fronting **themed tenant assets** with a CDN.
Specifically, today that means the per-tenant logo served by
`GET /v1/theme/logo` (added in migration `0020_tenant_theme_logo.sql`,
implemented in `server/src/routes/theme.rs::get_logo`). Future themed
assets — a per-tenant favicon and Open-Graph (`og:image`) endpoint
— will follow the same pattern; the cache + CSP guidance below is
written so it applies to all three.

This is **not** about skill bundles. Bundle storage — including the
per-tenant `storage_uri` override — is covered by
[`data-residency.md`](./data-residency.md). Bundles are downloaded
through `GET /v1/skills/.../bundle` with signed URLs (a different
auth and caching story); they do not share an edge layer with themed
assets and operators should think of them separately.

For the upstream theme model (schema, sanitization, contrast checks)
see [`../theming.md`](../theming.md).

## Why a CDN

Three reasons to front the themed-asset endpoints with a CDN:

- **Latency.** A logo is fetched on every page load — login page,
  catalog, every authed route. A CDN edge in the user's region is a
  one-hop image fetch instead of a multi-hop round trip to the
  origin.
- **Bandwidth.** Logos are small (256 KiB cap), but they multiply
  fast with active users. Pushing them onto a CDN frees origin
  bandwidth for the API surface that actually mutates state.
- **ACL simplification.** A CDN that fronts a public, cache-friendly
  endpoint lets you keep the origin endpoint behind a strict
  reverse-proxy ACL without breaking the user experience.
  `GET /v1/theme/logo` is already public (matches `GET /v1/theme`'s
  auth model — see the comment on `server/src/routes/theme.rs::get_logo`),
  so it is a natural CDN candidate.

## Signed URL vs public bucket

When a tenant moves to their own backing store (e.g. via per-tenant
`storage_uri`), there are two ways to serve the bytes through a CDN.
The tradeoffs:

| Concern                | Signed URL                                  | Public bucket + CDN                       |
|------------------------|---------------------------------------------|-------------------------------------------|
| **Security**           | Each URL carries an HMAC-signed expiry. A leaked URL becomes useless after the TTL. | Object is world-readable to anyone who guesses the key. Mitigated by long, unguessable keys (`{tenant_uuid}/theme/logo.png`) but still public. |
| **Edge caching**       | Hard. Each signed URL is unique, so the CDN cache key includes the signature; cache hit rate collapses. | Trivial. One URL per asset; cache key is stable; hit rate approaches 100%. |
| **Revocation cost**    | Cheap. Re-key or rotate the signer; existing URLs expire on their own. | Expensive. Object must be overwritten or deleted; CDN cache may also need a purge. |
| **CSP implications**   | The signed URL host must be in `img-src`. Per-tenant signing hosts would explode the CSP whitelist. | One stable host per tenant CDN — `cdn.acme.example.com` — added once to `img-src`. |
| **Operational simplicity** | Origin needs a signer; clients need to refresh URLs near expiry. | Origin uploads bytes; CDN handles everything else. |
| **Use case**           | One-off downloads (skill bundles, exports). | Logos, favicons, OG images — content that is *meant* to be public, frequently fetched, and rarely changed. |

For themed assets the public-bucket-plus-CDN pattern wins on every
metric except revocation, and themed-asset revocation is cheap in
practice (a tenant uploads a new logo over the old key — see "Cache
invalidation" below).

## Recommended pattern

### Free / Team tier — no CDN

Serve from `/v1/theme/logo` directly. The endpoint already returns
`Cache-Control: public, max-age=300` (see
`server/src/routes/theme.rs::get_logo`). A reverse proxy in front of
the API server (nginx, Caddy, Cloud-managed LB) honours that header
and edge-caches for five minutes. No CDN, no per-tenant DNS, no
extra CSP entries.

In rough numbers: a tenant with 1,000 weekly active users hitting
~10 logo fetches each = 10k requests/week or ~1.5/min — well below
the threshold where a CDN earns its operational cost.

### Enterprise tier with own `storage_uri` — CDN in front of the tenant bucket

When an Enterprise tenant has [data residency](./data-residency.md)
enabled (per-tenant `storage_uri` pointing at their own regional
bucket), front that bucket with a CDN they control:

1. Provision a CDN distribution (CloudFront, CloudFlare, Fastly,
   …) whose origin is the tenant bucket.
2. Point a CNAME at the distribution — e.g.
   `cdn.acme.example.com → d1234.cloudfront.net`.
3. Restrict the bucket to the CDN's origin identity (CloudFront OAC,
   CloudFlare R2 access binding, …). The bucket stays non-public;
   only the CDN can read it.
4. Update the tenant's `logo_uri` column to the CDN URL of their
   logo object (e.g.
   `https://cdn.acme.example.com/<tenant_uuid>/theme/logo.svg`).
   The portal client falls back to `logo_uri` whenever
   `logo_storage_key` is NULL (and you can leave the API-served
   endpoint disabled at the proxy if you prefer). See "Schema" in
   [`../theming.md`](../theming.md) for the column model.

This keeps the bytes in the tenant's region (data-residency story
intact) and serves them through their CDN of choice (latency story
optimised). The skill-pool origin sees zero traffic for logo
fetches once `logo_uri` is set.

## CSP implications

There is no `Content-Security-Policy` header set anywhere in
`server/src/routes/*.rs` today, and the SvelteKit web layer does not
emit one from `web/src/app.html` or `web/svelte.config.js`. Operators
who add a CSP at their reverse proxy — strongly recommended for
production — need to think about themed-asset hosts in the
following directives:

- **`img-src`** — the host serving `/v1/theme/logo` (or the CDN
  CNAME, if you went with the Enterprise pattern above) must be
  listed. For default deploys this is `'self'`. For tenants on a
  custom logo CDN, add their CNAME.
- **`style-src`** — needs `'unsafe-inline'` because the theme is
  injected as an inline `<style>` block by
  `web/src/routes/+layout.svelte` (see "Request-time resolution" in
  [`../theming.md`](../theming.md)). The alternative is a
  per-request nonce, which the SvelteKit hook would have to thread
  through; not implemented today.
- **`connect-src`** — only matters for the admin page's logo
  upload (it `POST`s multipart to the same origin); `'self'` is
  enough.

When an operator switches a tenant to a custom CDN they MUST add
the CDN's CNAME to `img-src` on every page that might render the
logo — i.e. every page, since the layout shows the brand on every
route. Forgetting this fails open: the page renders with the
default favicon and a broken image where the logo should be. The
console will show a CSP violation report.

A sample production CSP for a single-CDN deploy:

```
Content-Security-Policy:
  default-src 'self';
  img-src 'self' data: https://cdn.acme.example.com;
  style-src 'self' 'unsafe-inline';
  script-src 'self';
  connect-src 'self';
  frame-ancestors 'none';
```

Multi-tenant deploys with per-tenant CDNs need either (a) a CSP
generator that adds the tenant's CDN host to `img-src` per
request, or (b) a wildcard like `https://cdn.*.example.com` if your
CDN hostnames are predictable. SvelteKit `hooks.server.ts` is the
right place to do (a) — it already resolves the tenant per request
and could emit a tenant-specific CSP header at the same time.

## Data-residency interaction

Per-tenant `storage_uri` (see
[`data-residency.md`](./data-residency.md)) moves the logo blob
into the tenant's chosen storage backend. That backend is normally
chosen for **regional** reasons — an EU tenant pins their bundles
and logos to an `eu-west-1` bucket so the bytes never leave the
region.

The CDN edge layer should follow suit. A CDN that serves an
EU-hosted bucket from US edges defeats the residency story end-to-
end, because:

- The first request from any region populates that region's edge
  cache with bytes that *did* leave the EU even if the origin
  bucket stayed put.
- Compliance audits care about *where bytes are served from*, not
  just where they are at rest.

Practically this means:

- Pick a CDN that lets you scope edge POPs by region. CloudFront
  supports per-distribution price classes that limit edge locations.
  CloudFlare's [regional services](https://developers.cloudflare.com/data-localization/)
  bind cache to a set of geographies. Fastly has POP-selection
  knobs in its CLI.
- If your CDN can't scope edges, set `Cache-Control: private` (or
  `no-store`) on the asset so the CDN tunnels every request back to
  the origin in-region. You lose the latency win, but residency is
  preserved.
- The skill-pool server's per-tenant `storage_uri` value carries
  no region tag — the parallel `tenants.region` column is the
  metadata signal. Operators must keep the CDN region in sync with
  `tenants.region` manually; the server does not enforce or even
  check this.

Bundle delivery sits adjacent to this and is **out of scope for
this doc** — bundle bytes are streamed by the origin with their own
auth and signing model. See [`data-residency.md`](./data-residency.md)
for the bundle layer and the bucket-policy templates in
`packaging/bucket-policy/` for the storage-side ACL.

## Cache invalidation

When a tenant uploads a new logo:

1. `post_logo` in `server/src/routes/theme.rs` writes the sanitized
   bytes to the storage key returned by
   `server/src/storage.rs::Storage::logo_key`. The key shape is
   `{tenant_uuid}/theme/logo.{ext}`.
2. If the new upload is the **same extension** as the previous one
   (SVG → SVG, PNG → PNG), the object is overwritten **in place**
   — same key, same path. CDN edges see new bytes on the next
   revalidation.
3. If the new upload is a **different extension** (SVG → PNG,
   WEBP → SVG, …), the new object is written to the new key and
   the previous key is best-effort deleted. Cache-busting at this
   point is automatic because the URL itself changed (the file
   extension is part of the path).

With `Cache-Control: public, max-age=300` from
`server/src/routes/theme.rs::get_logo`, the worst-case staleness for
a same-extension replacement is **five minutes** end-to-end:

- Browser cache holds the old image for up to 5 min.
- CDN edge holds it for up to 5 min and revalidates against origin.

Five minutes is acceptable for branding (an admin changes a logo
because of a rebrand, not because of a live incident). Operators
who need faster propagation can:

- Reduce `max-age` at the reverse-proxy layer (override the
  server's header). 60 seconds is sustainable; below that the
  cache stops mattering.
- Issue a CDN purge via the provider's API after the
  `theme.logo.upload` audit event lands. CloudFront has
  `CreateInvalidation`; CloudFlare has the `/zones/.../purge_cache`
  endpoint. Wiring a purge requires an operator-side webhook
  subscriber on the audit stream — see `docs/audit.md` (and
  `server/migrations/0017_audit_siem.sql`) for the audit pipeline.

The admin theme page works around the cache for the **current admin
user** by appending a `?v={timestamp}` query string on the `<img>`
src after a successful upload (`logoBust` in
`web/src/routes/(authed)/admin/theme/+page.svelte`). Other users on
the tenant still see the old logo until their cache expires; the
busted URL is only used while the admin is on the page.

## Open follow-ups

The themed-asset story is small today (one endpoint, two CSP
directives, one cache header). The pattern documented above is
deliberately written for the assets that ship next:

- **Favicon endpoint.** A future `GET /v1/theme/favicon` will
  follow the same shape as the logo endpoint: public, magic-checked,
  same 256 KiB cap, same `Cache-Control: public, max-age=300`. The
  same CDN guidance applies. The favicon is reused by every browser
  tab and is *more* sensitive to cache TTL because users see it
  during multitasking; consider bumping `max-age` to one hour once
  the endpoint exists.
- **Open-Graph image endpoint.** A future `GET /v1/theme/og-image`
  needs a **longer** TTL (1+ day) because the consumers — Slack,
  Twitter/X, Discord, LinkedIn — fetch the URL once and cache it
  aggressively on their own infrastructure. A short TTL on the
  origin doesn't propagate to those caches anyway; using a query
  string version selector (`?v=<hash>`) is the only way to bust
  them. Expect the OG endpoint to ship with `Cache-Control: public,
  max-age=86400, immutable` plus a hash in the URL itself.
- **Per-tenant CSP generator.** A SvelteKit hook that emits a
  tenant-specific `Content-Security-Policy` header (so each
  tenant's CDN CNAME is on their `img-src` and nobody else's) is
  the natural next step once multi-tenant CDN deploys ship in
  practice. Today the recommendation is a single permissive
  wildcard or a hand-maintained allow-list.
- **Bundle-CDN extension.** Skill-bundle delivery via signed URL
  through a CDN is a separate spec (signed URLs are cache-hostile,
  see the comparison table above). It belongs alongside the
  download-counter and antibot work in
  [`data-residency.md`](./data-residency.md), not here.

## Related files and cross-links

- `server/src/routes/theme.rs` — the GET / POST / DELETE logo
  endpoints; `Cache-Control` header is on `get_logo`.
- `server/src/logo_sanitize.rs` — sanitization pipeline (referenced
  here only for context; see `../theming.md` for the full reject
  list).
- `server/src/storage.rs::Storage::logo_key` — the
  `{tenant_uuid}/theme/logo.{ext}` key shape.
- `server/migrations/0020_tenant_theme_logo.sql` — the columns and
  CHECK constraints.
- `web/src/routes/(authed)/admin/theme/+page.svelte` — admin-side
  cache-busting via `?v=<timestamp>`.
- [`../theming.md`](../theming.md) — full theme model: schema,
  CSS variables, contrast checks, sanitizer reject list.
- [`./data-residency.md`](./data-residency.md) — per-tenant
  `storage_uri`; regional considerations that the CDN layer must
  inherit.
- [`./custom-domains.md`](./custom-domains.md) — the tenant's
  custom hostname is the leftmost label the portal uses to resolve
  the theme; CDN CNAMEs typically sit alongside the custom domain.
- [`./branded-emails.md`](./branded-emails.md) — branded
  transactional email reuses the same `brand_name` and is the only
  themed surface outside the web portal today.
