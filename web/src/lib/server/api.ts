/**
 * Server-side HTTP client for skill-pool-server.
 *
 * Lives under `src/lib/server/` so SvelteKit guarantees the module is never
 * bundled into the client — keeps tokens off the network and lets us trust
 * the API base URL from environment alone.
 */

import { env } from '$env/dynamic/private';
import type { Skill } from '$lib/types';

const DEFAULT_API_BASE = 'http://127.0.0.1:8080';

function base(): string {
  return env.SKILL_POOL_API_BASE?.replace(/\/$/, '') ?? DEFAULT_API_BASE;
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
  ) {
    super(message);
  }
}

export interface Auth {
  tenant: string;
  token?: string;
}

async function call(
  method: string,
  path: string,
  auth: Auth,
  init?: RequestInit & { jsonBody?: unknown },
): Promise<Response> {
  const headers = new Headers(init?.headers);
  headers.set('X-Skill-Pool-Tenant', auth.tenant);
  if (auth.token) headers.set('Authorization', `Bearer ${auth.token}`);
  if (init?.jsonBody !== undefined) {
    headers.set('Content-Type', 'application/json');
  }
  const body =
    init?.jsonBody !== undefined ? JSON.stringify(init.jsonBody) : init?.body;
  return fetch(`${base()}${path}`, { method, headers, body });
}

export async function listSkills(auth: Auth, query?: string): Promise<Skill[]> {
  const params = new URLSearchParams();
  if (query) params.set('query', query);
  const url = `/v1/skills${params.size ? '?' + params : ''}`;
  const resp = await call('GET', url, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getSkill(auth: Auth, slug: string): Promise<Skill> {
  const resp = await call('GET', `/v1/skills/${encodeURIComponent(slug)}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function pingHealth(auth: Auth): Promise<boolean> {
  try {
    const resp = await call('GET', '/v1/healthz', auth);
    return resp.ok;
  } catch {
    return false;
  }
}

/** Lightweight check: the token authenticates against /v1/skills for this tenant. */
export async function validateAuth(auth: Auth): Promise<boolean> {
  try {
    const resp = await call('GET', '/v1/skills?limit=1', auth);
    return resp.ok;
  } catch {
    return false;
  }
}
