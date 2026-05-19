// SvelteKit proxy for `GET /v1/theme/logo`.
//
// The API server resolves the tenant from `X-Skill-Pool-Tenant`, which the
// browser can't set on a plain `<img src=...>`. The page renders against
// this local route instead; we forward the request to the API and stream
// the response bytes back, preserving Content-Type and Cache-Control.

import { error } from '@sveltejs/kit';
import type { RequestHandler } from './$types';

const DEFAULT_API_BASE = 'http://127.0.0.1:8080';

function apiBase(): string {
  // Same logic as $lib/server/api.ts but inlined to keep this file
  // independent of any module that does a top-level env read.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const env = (globalThis as any).process?.env ?? {};
  return (env.SKILL_POOL_API_BASE as string | undefined)?.replace(/\/$/, '') ?? DEFAULT_API_BASE;
}

export const GET: RequestHandler = async ({ locals }) => {
  const resp = await fetch(`${apiBase()}/v1/theme/logo`, {
    headers: { 'X-Skill-Pool-Tenant': locals.tenant.slug },
  });
  if (resp.status === 404) {
    throw error(404, 'no logo set');
  }
  if (!resp.ok) {
    throw error(resp.status, await resp.text());
  }
  const ct = resp.headers.get('content-type') ?? 'application/octet-stream';
  const cc = resp.headers.get('cache-control') ?? 'public, max-age=300';
  // Stream the body straight through.
  return new Response(resp.body, {
    status: 200,
    headers: { 'Content-Type': ct, 'Cache-Control': cc },
  });
};
