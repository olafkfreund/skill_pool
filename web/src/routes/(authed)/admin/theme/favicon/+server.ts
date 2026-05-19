// SvelteKit proxy for `GET /v1/theme/favicon`.
//
// Mirrors the `/admin/theme/logo` proxy: the admin page's preview can't set
// `X-Skill-Pool-Tenant` on a bare `<img src=...>`, so the request travels
// through this server route which forwards to the API.
//
// The API server itself handles the fallback behaviour — when no favicon is
// uploaded but a logo is, it returns the logo bytes here. The proxy just
// streams whatever the API hands back.

import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

const DEFAULT_API_BASE = 'http://127.0.0.1:8080';

function apiBase(): string {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const env = (globalThis as any).process?.env ?? {};
  return (env.SKILL_POOL_API_BASE as string | undefined)?.replace(/\/$/, '') ?? DEFAULT_API_BASE;
}

export const GET: RequestHandler = async ({ locals }) => {
  const resp = await fetch(`${apiBase()}/v1/theme/favicon`, {
    headers: { 'X-Skill-Pool-Tenant': locals.tenant.slug },
  });
  if (resp.status === 404) {
    throw error(404, 'no favicon set');
  }
  if (!resp.ok) {
    throw error(resp.status, await resp.text());
  }
  const ct = resp.headers.get('content-type') ?? 'application/octet-stream';
  const cc = resp.headers.get('cache-control') ?? 'public, max-age=300';
  return new Response(resp.body, {
    status: 200,
    headers: { 'Content-Type': ct, 'Cache-Control': cc },
  });
};
