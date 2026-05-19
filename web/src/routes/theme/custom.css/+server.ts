// SvelteKit proxy for `GET /v1/theme/custom.css`.
//
// The root layout injects `<link rel="stylesheet" href="/theme/custom.css">`
// when the tenant has an overlay set. Browsers can't add `X-Skill-Pool-Tenant`
// to a bare `<link>` request, so we forward through this server route which
// derives the tenant from `locals.tenant.slug` (populated by hooks.server.ts
// on every request — authed and public alike) and hits the API.
//
// The route lives OUTSIDE the `(authed)` group on purpose: the custom CSS is
// part of the brand-presentation surface (login page included). The API
// already pins `Content-Security-Policy: style-src 'self'` on its response;
// we preserve those headers when forwarding so the browser sees the same
// guarantees here.

import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

const DEFAULT_API_BASE = 'http://127.0.0.1:8080';

function apiBase(): string {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const env = (globalThis as any).process?.env ?? {};
  return (env.SKILL_POOL_API_BASE as string | undefined)?.replace(/\/$/, '') ?? DEFAULT_API_BASE;
}

export const GET: RequestHandler = async ({ locals }) => {
  const resp = await fetch(`${apiBase()}/v1/theme/custom.css`, {
    headers: { 'X-Skill-Pool-Tenant': locals.tenant.slug },
  });
  if (resp.status === 404) {
    throw error(404, 'no custom CSS set');
  }
  if (!resp.ok) {
    throw error(resp.status, await resp.text());
  }
  // Preserve the API server's headers — Content-Type, Cache-Control,
  // CSP, and nosniff are all set there. We re-emit known good defaults
  // for the headers that might be missing on a misconfigured backend.
  const ct = resp.headers.get('content-type') ?? 'text/css; charset=utf-8';
  const cc = resp.headers.get('cache-control') ?? 'public, max-age=300';
  const csp = resp.headers.get('content-security-policy') ?? "style-src 'self'";
  const xcto = resp.headers.get('x-content-type-options') ?? 'nosniff';
  return new Response(resp.body, {
    status: 200,
    headers: {
      'Content-Type': ct,
      'Cache-Control': cc,
      'Content-Security-Policy': csp,
      'X-Content-Type-Options': xcto,
    },
  });
};
