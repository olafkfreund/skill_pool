/**
 * Server-side HTTP client for skill-pool-server.
 *
 * Lives under `src/lib/server/` so SvelteKit guarantees the module is never
 * bundled into the client — keeps tokens off the network and lets us trust
 * the API base URL from environment alone.
 */

import { env } from '$env/dynamic/private';
import type { Skill } from '$lib/types';
import type { Theme } from '$lib/theme';

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
  const body = init?.jsonBody !== undefined ? JSON.stringify(init.jsonBody) : init?.body;
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

export async function getSkillMd(auth: Auth, slug: string): Promise<string> {
  const resp = await call('GET', `/v1/skills/${encodeURIComponent(slug)}/skill-md`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.text();
}

export interface ServerTheme {
  brand_name: string;
  primary: string;
  primary_fg: string;
  accent: string;
  bg: string;
  fg: string;
  muted: string;
  muted_fg: string;
  border: string;
  radius: string;
  logo_uri?: string | null;
}

export async function getTheme(auth: Auth): Promise<ServerTheme | null> {
  const resp = await call('GET', '/v1/theme', auth);
  if (!resp.ok) return null;
  return resp.json();
}

export async function putTheme(
  auth: Auth,
  theme: ServerTheme,
): Promise<{ ok: boolean; status: number; error?: string }> {
  const resp = await call('PUT', '/v1/theme', auth, { jsonBody: theme });
  if (resp.ok) return { ok: true, status: resp.status };
  const error = await resp.text();
  return { ok: false, status: resp.status, error };
}

/** Server-side translation helper — fold the API shape into the client's. */
export function toClientTheme(s: ServerTheme): Theme {
  return {
    brandName: s.brand_name,
    primary: s.primary,
    primaryFg: s.primary_fg,
    accent: s.accent,
    bg: s.bg,
    fg: s.fg,
    muted: s.muted,
    mutedFg: s.muted_fg,
    border: s.border,
    radius: s.radius,
    logoUrl: s.logo_uri ?? undefined,
  };
}

/** Inverse of toClientTheme, used when saving from the editor. */
export function fromClientTheme(t: Theme): ServerTheme {
  return {
    brand_name: t.brandName,
    primary: t.primary,
    primary_fg: t.primaryFg,
    accent: t.accent,
    bg: t.bg,
    fg: t.fg,
    muted: t.muted,
    muted_fg: t.mutedFg,
    border: t.border,
    radius: t.radius,
    logo_uri: t.logoUrl,
  };
}

export interface PublishMetadata {
  slug: string;
  version: string;
  when_to_use?: string;
  tags?: string[];
}

export interface ValidationResult {
  ok: boolean;
  /** Server-reported error on failure (frontmatter, secret scan, etc.). */
  error?: string;
  /** Echoed metadata on success. */
  name?: string;
  description?: string;
  tags?: string[];
}

async function multipartCall(
  method: string,
  path: string,
  auth: Auth,
  metadata: PublishMetadata | undefined,
  bundle: Uint8Array,
): Promise<Response> {
  const form = new FormData();
  if (metadata !== undefined) {
    form.append('metadata', JSON.stringify(metadata));
  }
  // Blob copy is unavoidable for the FormData type; bundles are small (<5 MB).
  form.append('bundle', new Blob([bundle.slice()], { type: 'application/gzip' }), 'skill.tar.gz');

  const headers = new Headers();
  headers.set('X-Skill-Pool-Tenant', auth.tenant);
  if (auth.token) headers.set('Authorization', `Bearer ${auth.token}`);

  return fetch(`${base()}${path}`, { method, headers, body: form });
}

export async function validateSkill(auth: Auth, bundle: Uint8Array): Promise<ValidationResult> {
  const resp = await multipartCall('POST', '/v1/skills/validate', auth, undefined, bundle);
  if (resp.ok) {
    const j = (await resp.json()) as ValidationResult;
    return { ...j, ok: true };
  }
  const error = await resp.text();
  return { ok: false, error };
}

export async function publishSkill(
  auth: Auth,
  metadata: PublishMetadata,
  bundle: Uint8Array,
): Promise<{ ok: true; skill: Skill } | { ok: false; status: number; error: string }> {
  const resp = await multipartCall('POST', '/v1/skills', auth, metadata, bundle);
  if (resp.ok) {
    return { ok: true, skill: (await resp.json()) as Skill };
  }
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function discoverOidc(auth: Auth): Promise<{ enabled: boolean }> {
  const resp = await call('GET', '/v1/auth/oidc/discover', auth);
  if (!resp.ok) return { enabled: false };
  return resp.json();
}

export async function discoverSaml(auth: Auth): Promise<{ enabled: boolean }> {
  const resp = await call('GET', '/v1/auth/saml/discover', auth);
  if (!resp.ok) return { enabled: false };
  return resp.json();
}

/** Build the `?return_to=` URL the server redirects back to once OIDC completes. */
export function oidcStartUrl(tenant: string, returnTo: string): string {
  const url = `${base()}/v1/auth/oidc/${encodeURIComponent(tenant)}/start`;
  const params = new URLSearchParams({ return_to: returnTo });
  return `${url}?${params}`;
}

/**
 * SAML is IdP-initiated for v1 — there's no SP-initiated AuthnRequest yet.
 * The user goes to the IdP's SSO URL directly; the IdP POSTs an assertion to
 * our ACS endpoint. Resolves to `null` until SAML config exposes the URL.
 */
export async function samlIdpUrl(auth: Auth): Promise<string | null> {
  // We deliberately don't expose the IdP URL via API — admins paste it into
  // the portal's hosted help text. For now return null and rely on doc.
  return Promise.resolve(null);
}

/** URL of our SP metadata, useful to surface in admin UI for IdP imports. */
export function samlMetadataUrl(tenant: string): string {
  return `${base()}/v1/auth/saml/${encodeURIComponent(tenant)}/metadata`;
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
