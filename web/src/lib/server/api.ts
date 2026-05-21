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

/** Catalog item kind discriminator. Slice 1 added this to the server. */
export type CatalogKind = 'skill' | 'agent' | 'command';
export const CATALOG_KINDS: CatalogKind[] = ['skill', 'agent', 'command'];

export function isCatalogKind(v: string | null | undefined): v is CatalogKind {
  return v === 'skill' || v === 'agent' || v === 'command';
}

/** Append `?kind=` only when not the default to keep URLs tidy. */
function kindQuery(kind: CatalogKind | undefined): string {
  return kind && kind !== 'skill' ? `?kind=${kind}` : '';
}

export async function listSkills(
  auth: Auth,
  options: {
    query?: string;
    semantic?: string;
    minSimilarity?: number;
    limit?: number;
    kind?: CatalogKind;
  } = {},
): Promise<Skill[]> {
  const params = new URLSearchParams();
  if (options.semantic) {
    params.set('semantic', options.semantic);
    if (options.minSimilarity !== undefined) {
      params.set('min_similarity', String(options.minSimilarity));
    }
  } else if (options.query) {
    params.set('query', options.query);
  }
  if (options.limit !== undefined) params.set('limit', String(options.limit));
  if (options.kind && options.kind !== 'skill') params.set('kind', options.kind);
  const url = `/v1/skills${params.size ? '?' + params : ''}`;
  const resp = await call('GET', url, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getSkill(auth: Auth, slug: string, kind?: CatalogKind): Promise<Skill> {
  const resp = await call('GET', `/v1/skills/${encodeURIComponent(slug)}${kindQuery(kind)}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getSkillMd(auth: Auth, slug: string, kind?: CatalogKind): Promise<string> {
  const resp = await call(
    'GET',
    `/v1/skills/${encodeURIComponent(slug)}/skill-md${kindQuery(kind)}`,
    auth,
  );
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.text();
}

export interface DependentEntry {
  slug: string;
  version: string;
  version_range: string;
}

export interface PendingMergeProposal {
  draft_id: string;
  draft_slug: string;
  similarity: number | null;
}

export interface SkillDetail {
  slug: string;
  version: string;
  description: string;
  when_to_use: string | null;
  tags: string[];
  status: string;
  created_at: string;
  use_count: number;
  last_used_at: string | null;
  requires: DependentEntry[];
  required_by: DependentEntry[];
  merge_proposals: PendingMergeProposal[];
}

export async function getSkillDetail(
  auth: Auth,
  slug: string,
  kind?: CatalogKind,
): Promise<SkillDetail> {
  const resp = await call(
    'GET',
    `/v1/skills/${encodeURIComponent(slug)}/detail${kindQuery(kind)}`,
    auth,
  );
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

/** One row of the per-skill version history. */
export interface SkillVersion {
  version: string;
  published_at: string;
  /** Email of the publishing user. Omitted by the server when null. */
  published_by?: string;
  /** Description truncated to 200 chars (the server has no separate column). */
  change_summary: string;
  status: string;
}

export async function listSkillVersions(
  auth: Auth,
  slug: string,
  kind?: CatalogKind,
): Promise<SkillVersion[]> {
  const resp = await call(
    'GET',
    `/v1/skills/${encodeURIComponent(slug)}/versions${kindQuery(kind)}`,
    auth,
  );
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export interface Member {
  id: string;
  email: string;
  display_name?: string | null;
  role: 'viewer' | 'publisher' | 'curator' | 'admin';
  joined_at: string;
  active: boolean;
}

export async function listMembers(auth: Auth): Promise<Member[]> {
  const resp = await call('GET', '/v1/tenant/members', auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function patchMemberRole(
  auth: Auth,
  id: string,
  role: Member['role'],
): Promise<{ ok: true; member: Member } | { ok: false; status: number; error: string }> {
  const resp = await call('PATCH', `/v1/tenant/members/${encodeURIComponent(id)}`, auth, {
    jsonBody: { role },
  });
  if (resp.ok) return { ok: true, member: (await resp.json()) as Member };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function removeMember(
  auth: Auth,
  id: string,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', `/v1/tenant/members/${encodeURIComponent(id)}`, auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
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
  /** Whether the "Powered by skill-pool" footer credit is shown. Defaults to true. */
  footer_branding: boolean;
  /** Selected font from the server-side allowlist, or absent for system. */
  font_family?: string | null;
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

/**
 * Upload a tenant logo. The server sanitizes the bytes (`logo_sanitize`) and
 * stores them under a per-tenant key. Returns the freshly-updated theme on
 * success.
 *
 * Accepted MIME types match the server's allow-list: `image/svg+xml`,
 * `image/png`, `image/jpeg`, `image/webp`. 256 KiB cap.
 */
export async function uploadLogo(
  auth: Auth,
  file: File,
): Promise<{ ok: true; theme: ServerTheme } | { ok: false; status: number; error: string }> {
  const form = new FormData();
  form.append('file', file, file.name);

  const headers = new Headers();
  headers.set('X-Skill-Pool-Tenant', auth.tenant);
  if (auth.token) headers.set('Authorization', `Bearer ${auth.token}`);

  const resp = await fetch(`${base()}/v1/theme/logo`, {
    method: 'POST',
    headers,
    body: form,
  });
  if (resp.ok) return { ok: true, theme: (await resp.json()) as ServerTheme };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/** Delete the tenant's uploaded logo. 204 on success. */
export async function deleteLogo(
  auth: Auth,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', '/v1/theme/logo', auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/**
 * Public URL for the tenant's logo. The endpoint is tenant-resolved via the
 * `X-Skill-Pool-Tenant` header server-side; in the browser we just point an
 * `<img>` at it and let SvelteKit's proxy / the API gateway route by host.
 */
export function logoUrl(): string {
  return `${base()}/v1/theme/logo`;
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
    footerBranding: s.footer_branding ?? true,
    fontFamily: s.font_family ?? undefined,
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
    footer_branding: t.footerBranding,
    font_family: t.fontFamily,
  };
}

/**
 * Fetch the curated font allowlist from the API. Returns `null` when the
 * call fails so callers can fall back to a hard-coded list rather than
 * blocking page load.
 */
export async function getFonts(auth: Auth): Promise<string[] | null> {
  const resp = await call('GET', '/v1/theme/fonts', auth);
  if (!resp.ok) return null;
  const body = (await resp.json()) as { allowed?: string[] };
  return body.allowed ?? null;
}

/**
 * Upload a tenant favicon. Same sanitizer pipeline as the logo plus
 * `image/x-icon`. 64 KiB cap (smaller than the logo's 256 KiB).
 */
export async function uploadFavicon(
  auth: Auth,
  file: File,
): Promise<{ ok: true; theme: ServerTheme } | { ok: false; status: number; error: string }> {
  const form = new FormData();
  form.append('file', file, file.name);

  const headers = new Headers();
  headers.set('X-Skill-Pool-Tenant', auth.tenant);
  if (auth.token) headers.set('Authorization', `Bearer ${auth.token}`);

  const resp = await fetch(`${base()}/v1/theme/favicon`, {
    method: 'POST',
    headers,
    body: form,
  });
  if (resp.ok) return { ok: true, theme: (await resp.json()) as ServerTheme };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/** Delete the tenant's uploaded favicon. 204 on success. */
export async function deleteFavicon(
  auth: Auth,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', '/v1/theme/favicon', auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/**
 * Upload a tenant custom-CSS overlay. The server sanitizes the bytes
 * (`css_sanitize`) — rejects `@import`, off-site `url()`, `expression()`,
 * `behavior:`, `javascript:` URIs, HTML-tag-like content, and the literal
 * `</style>` sequence. 32 KiB cap.
 *
 * `css` is the raw stylesheet text (we wrap it into a `Blob` and submit as
 * multipart so the server's existing extractor pattern keeps working).
 */
export async function uploadCustomCss(
  auth: Auth,
  css: string,
): Promise<{ ok: true; theme: ServerTheme } | { ok: false; status: number; error: string }> {
  const blob = new Blob([css], { type: 'text/css' });
  const form = new FormData();
  form.append('file', blob, 'overlay.css');

  const headers = new Headers();
  headers.set('X-Skill-Pool-Tenant', auth.tenant);
  if (auth.token) headers.set('Authorization', `Bearer ${auth.token}`);

  const resp = await fetch(`${base()}/v1/theme/custom-css`, {
    method: 'POST',
    headers,
    body: form,
  });
  if (resp.ok) return { ok: true, theme: (await resp.json()) as ServerTheme };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/** Delete the tenant's custom-CSS overlay. 204 on success. */
export async function deleteCustomCss(
  auth: Auth,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', '/v1/theme/custom-css', auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/**
 * Fetch the raw CSS bytes for the editor. Returns the text on 200,
 * `null` on 404 (no overlay), and throws on other failures.
 */
export async function fetchCustomCss(auth: Auth): Promise<string | null> {
  const resp = await call('GET', '/v1/theme/custom.css', auth);
  if (resp.status === 404) return null;
  if (!resp.ok) throw new Error(`fetch custom CSS failed: ${resp.status}`);
  return await resp.text();
}

export interface PublishMetadata {
  slug: string;
  version: string;
  when_to_use?: string;
  tags?: string[];
  /** Slice 1 added this. Defaults to `skill` on the server side. */
  kind?: CatalogKind;
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

export interface Draft {
  id: string;
  slug: string;
  description: string;
  when_to_use: string | null;
  tags: string[];
  origin: 'cli' | 'capture-scorer' | 'claude-hook' | 'web';
  notes: string | null;
  status: 'pending' | 'published' | 'discarded';
  published_version: string | null;
  created_at: string;
  reviewed_at: string | null;
  /** When set, this draft's description was within DEDUP_THRESHOLD of the named skill. */
  merge_proposal_slug?: string | null;
  merge_proposal_similarity?: number | null;
}

export async function listDrafts(
  auth: Auth,
  status: 'pending' | 'published' | 'discarded' | 'all' = 'pending',
): Promise<Draft[]> {
  const resp = await call('GET', `/v1/drafts?status=${status}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getDraft(auth: Auth, id: string): Promise<Draft> {
  const resp = await call('GET', `/v1/drafts/${encodeURIComponent(id)}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getDraftSkillMd(auth: Auth, id: string): Promise<string> {
  const resp = await call('GET', `/v1/drafts/${encodeURIComponent(id)}/skill-md`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.text();
}

export async function publishDraft(
  auth: Auth,
  id: string,
  version: string,
  slug?: string,
): Promise<
  | { ok: true; skill_id: string; slug: string; version: string }
  | { ok: false; status: number; error: string }
> {
  const body: { version: string; slug?: string } = { version };
  if (slug) body.slug = slug;
  const resp = await call('POST', `/v1/drafts/${encodeURIComponent(id)}/publish`, auth, {
    jsonBody: body,
  });
  if (resp.ok) {
    const j = (await resp.json()) as { skill_id: string; slug: string; version: string };
    return { ok: true, ...j };
  }
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function discardDraft(
  auth: Auth,
  id: string,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('POST', `/v1/drafts/${encodeURIComponent(id)}/discard`, auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export interface PatchDraftBody {
  slug?: string;
  description?: string;
  when_to_use?: string | null;
  tags?: string[];
  notes?: string | null;
}

export async function patchDraft(
  auth: Auth,
  id: string,
  body: PatchDraftBody,
): Promise<{ ok: true; draft: Draft } | { ok: false; status: number; error: string }> {
  const resp = await call('PATCH', `/v1/drafts/${encodeURIComponent(id)}`, auth, {
    jsonBody: body,
  });
  if (resp.ok) return { ok: true, draft: (await resp.json()) as Draft };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export interface NotificationsConfig {
  webhook_url?: string | null;
  signing_enabled: boolean;
  smtp_url?: string | null;
  smtp_from?: string | null;
  smtp_to?: string | null;
}

export async function getNotifications(auth: Auth): Promise<NotificationsConfig | null> {
  const resp = await call('GET', '/v1/tenant/notifications', auth);
  if (!resp.ok) return null;
  return resp.json();
}

export interface PutNotificationsBody {
  webhook_url?: string | null;
  webhook_secret?: string | null;
  smtp_url?: string | null;
  smtp_from?: string | null;
  smtp_to?: string | null;
}

export async function putNotifications(
  auth: Auth,
  body: PutNotificationsBody,
): Promise<
  { ok: true; config: NotificationsConfig } | { ok: false; status: number; error: string }
> {
  const resp = await call('PUT', '/v1/tenant/notifications', auth, { jsonBody: body });
  if (resp.ok) return { ok: true, config: (await resp.json()) as NotificationsConfig };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export interface DecayCandidate {
  slug: string;
  version: string;
  description: string;
  use_count: number;
  last_used_at: string | null;
  created_at: string;
}

export async function listDecayCandidates(
  auth: Auth,
  opts: { days?: number; maxUses?: number; limit?: number } = {},
): Promise<DecayCandidate[]> {
  const params = new URLSearchParams();
  if (opts.days !== undefined) params.set('days', String(opts.days));
  if (opts.maxUses !== undefined) params.set('max_uses', String(opts.maxUses));
  if (opts.limit !== undefined) params.set('limit', String(opts.limit));
  const url = `/v1/tenant/skills/decay${params.size ? '?' + params : ''}`;
  const resp = await call('GET', url, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function archiveSkill(
  auth: Auth,
  slug: string,
  kind?: CatalogKind,
): Promise<
  { ok: true; slug: string; version: string } | { ok: false; status: number; error: string }
> {
  const resp = await call(
    'POST',
    `/v1/skills/${encodeURIComponent(slug)}/archive${kindQuery(kind)}`,
    auth,
  );
  if (resp.ok) {
    const j = (await resp.json()) as { slug: string; version: string };
    return { ok: true, ...j };
  }
  return { ok: false, status: resp.status, error: await resp.text() };
}

export interface TimelineBucket {
  day: string;
  downloads: number;
  views: number;
  unique_skills: number;
}

export interface TopSkillRow {
  slug: string;
  downloads: number;
  views: number;
  total: number;
}

export interface StackMapping {
  stack: string;
  skill: string;
}

export async function listStackMappings(auth: Auth): Promise<StackMapping[]> {
  const resp = await call('GET', '/v1/tenant/stack-mappings', auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function upsertStackMapping(
  auth: Auth,
  body: StackMapping,
): Promise<{ ok: true; mapping: StackMapping } | { ok: false; status: number; error: string }> {
  const resp = await call('POST', '/v1/tenant/stack-mappings', auth, { jsonBody: body });
  if (resp.ok) return { ok: true, mapping: (await resp.json()) as StackMapping };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function removeStackMapping(
  auth: Auth,
  body: StackMapping,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', '/v1/tenant/stack-mappings', auth, { jsonBody: body });
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function getUsageTimeline(auth: Auth, days: number): Promise<TimelineBucket[]> {
  const resp = await call('GET', `/v1/tenant/usage/timeline?days=${days}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getUsageTop(auth: Auth, days: number, limit = 10): Promise<TopSkillRow[]> {
  const resp = await call('GET', `/v1/tenant/usage/top?days=${days}&limit=${limit}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function pendingDraftsCount(auth: Auth): Promise<number> {
  const resp = await call('GET', '/v1/tenant/notifications/pending-count', auth);
  if (!resp.ok) return 0;
  const body = (await resp.json()) as { pending?: number };
  return typeof body.pending === 'number' ? body.pending : 0;
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

// --- Admin SSO config CRUD (#4) ------------------------------------------

export interface SsoOidcView {
  issuer_url: string;
  client_id: string;
  /** Last 4 chars of the stored secret, prefixed with `••••`. Never plaintext. */
  client_secret_hint: string;
  default_role: SsoRole;
}

export interface SsoSamlView {
  idp_entity_id: string;
  idp_sso_url: string;
  idp_x509_cert_bytes: number;
  sp_entity_id?: string | null;
  default_role: SsoRole;
}

// Re-exported from the public `$lib/sso-roles` module so `+page.svelte`
// can import them without crossing the `$lib/server/*` boundary.
import type { SsoRole as _SsoRole } from '$lib/sso-roles';
export type SsoRole = _SsoRole;
export { SSO_ROLES } from '$lib/sso-roles';

export interface SsoConfigView {
  kind: 'oidc' | 'saml' | null;
  oidc?: SsoOidcView | null;
  saml?: SsoSamlView | null;
  scim_endpoint: string;
}

export async function getSsoConfig(auth: Auth): Promise<SsoConfigView | null> {
  const resp = await call('GET', '/v1/tenant/sso', auth);
  if (!resp.ok) return null;
  return resp.json();
}

export interface PutSsoOidcBody {
  issuer_url: string;
  client_id: string;
  client_secret: string;
  default_role: SsoRole;
}

export async function putSsoOidc(
  auth: Auth,
  body: PutSsoOidcBody,
): Promise<
  | { ok: true; config: SsoConfigView }
  | { ok: false; status: number; error: string }
> {
  const resp = await call('PUT', '/v1/tenant/sso/oidc', auth, { jsonBody: body });
  if (resp.ok) return { ok: true, config: (await resp.json()) as SsoConfigView };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export interface PutSsoSamlBody {
  metadata_xml: string;
  default_role: SsoRole;
  sp_entity_id?: string | null;
}

export async function putSsoSaml(
  auth: Auth,
  body: PutSsoSamlBody,
): Promise<
  | { ok: true; config: SsoConfigView }
  | { ok: false; status: number; error: string }
> {
  const resp = await call('PUT', '/v1/tenant/sso/saml', auth, { jsonBody: body });
  if (resp.ok) return { ok: true, config: (await resp.json()) as SsoConfigView };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function deleteSsoConfig(
  auth: Auth,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', '/v1/tenant/sso', auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
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

/**
 * Per-tenant session policy. Called by the login action so a tenant with
 * a stricter idle-timeout (e.g. 1 hour) sees that maxAge on their cookie.
 * Falls back to 14 days if the endpoint or call fails — never throws.
 */
const FALLBACK_SESSION_MAX_AGE = 60 * 60 * 24 * 14;
export async function getSessionMaxAge(tenant: string): Promise<number> {
  try {
    const resp = await call('GET', '/v1/tenant/session-policy', { tenant });
    if (!resp.ok) return FALLBACK_SESSION_MAX_AGE;
    const body = (await resp.json()) as { max_age_secs: number };
    const n = Number(body?.max_age_secs);
    if (!Number.isFinite(n) || n < 60 || n > 60 * 60 * 24 * 30) {
      return FALLBACK_SESSION_MAX_AGE;
    }
    return Math.floor(n);
  } catch {
    return FALLBACK_SESSION_MAX_AGE;
  }
}

/**
 * One row in the custom-domain admin table. Wire shape mirrors
 * `server/src/routes/custom_domains.rs::CustomDomain` — `verification_record`
 * is the pre-formatted "host TXT value" string the tenant pastes into their
 * DNS panel.
 *
 * `status` is one of `pending` | `verified` | `active` | `failed`. The server
 * stores `verified` after DNS-TXT control is proven; `active` is set by the
 * operator after the reverse proxy is wired up. From the UI's perspective both
 * are "good"; `pending` and `failed` are "needs attention".
 */
export interface CustomDomain {
  id: string;
  hostname: string;
  status: 'pending' | 'verified' | 'active' | 'failed' | string;
  verification_record: string;
  last_checked_at: string | null;
  last_error: string | null;
  activated_at: string | null;
  created_at: string;
}

/** GET /v1/tenant/custom-domains — list this tenant's claims. */
export async function listCustomDomains(auth: Auth): Promise<CustomDomain[]> {
  const resp = await call('GET', '/v1/tenant/custom-domains', auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

/**
 * POST /v1/tenant/custom-domains — claim a hostname. The server validates
 * the hostname shape and returns the row plus a `verification_record` string
 * the admin pastes into their DNS panel.
 */
export async function createCustomDomain(
  auth: Auth,
  hostname: string,
): Promise<{ ok: true; domain: CustomDomain } | { ok: false; status: number; error: string }> {
  const resp = await call('POST', '/v1/tenant/custom-domains', auth, {
    jsonBody: { hostname },
  });
  if (resp.ok) return { ok: true, domain: (await resp.json()) as CustomDomain };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/**
 * POST /v1/tenant/custom-domains/{id}/verify — runs the upstream TXT lookup
 * and flips the row to `verified` (on match) or `failed` (otherwise). The
 * fresh row is returned in both cases; on failure `last_error` carries the
 * resolver's message so the UI can surface it inline.
 */
export async function verifyCustomDomain(
  auth: Auth,
  id: string,
): Promise<{ ok: true; domain: CustomDomain } | { ok: false; status: number; error: string }> {
  const resp = await call(
    'POST',
    `/v1/tenant/custom-domains/${encodeURIComponent(id)}/verify`,
    auth,
  );
  if (resp.ok) return { ok: true, domain: (await resp.json()) as CustomDomain };
  return { ok: false, status: resp.status, error: await resp.text() };
}

/** DELETE /v1/tenant/custom-domains/{id} — withdraw a claim. 204 on success. */
export async function removeCustomDomain(
  auth: Auth,
  id: string,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', `/v1/tenant/custom-domains/${encodeURIComponent(id)}`, auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

// --- Profile / personal API tokens (#4) ----------------------------------

export interface WhoAmI {
  user_id: string;
  email: string;
  role: 'viewer' | 'publisher' | 'curator' | 'admin';
  tenant: string;
}

/** Identity of the calling user. Returns null when the caller isn't a
 *  session-authenticated user (e.g. plain CLI token). */
export async function whoami(auth: Auth): Promise<WhoAmI | null> {
  const resp = await call('GET', '/v1/auth/whoami', auth);
  if (!resp.ok) return null;
  return resp.json();
}

export interface ApiToken {
  id: string;
  label: string;
  /** First ~12 chars of the raw token. `null` for tokens minted before
   *  the prefix was tracked. */
  prefix: string | null;
  /** Space-separated scope string, e.g. `"skills:read skills:publish"`. */
  scopes: string;
  created_at: string;
  last_used_at: string | null;
  revoked_at: string | null;
}

/** Shown ONCE, only in the response to `createToken`. Never appears in
 *  `listTokens`. The web layer is responsible for surfacing it to the user
 *  immediately and then dropping it on the floor. */
export interface CreatedApiToken {
  id: string;
  raw_token: string;
  prefix: string;
  scopes: string;
  created_at: string;
  label: string;
}

export async function listMyTokens(auth: Auth): Promise<ApiToken[]> {
  const resp = await call('GET', '/v1/profile/tokens', auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function createMyToken(
  auth: Auth,
  label: string,
  scopes: string[],
): Promise<
  { ok: true; token: CreatedApiToken } | { ok: false; status: number; error: string }
> {
  const resp = await call('POST', '/v1/profile/tokens', auth, {
    jsonBody: { label, scopes },
  });
  if (resp.ok) return { ok: true, token: (await resp.json()) as CreatedApiToken };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function revokeMyToken(
  auth: Auth,
  id: string,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', `/v1/profile/tokens/${encodeURIComponent(id)}`, auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

// --- Admin Projects CRUD -------------------------------------------------

export interface Project {
  slug: string;
  name: string;
  description: string | null;
  git_remote: string | null;
  stack_tags: string[];
  /** Item count returned by the list endpoint for display purposes. */
  item_count?: number;
  /** Auto-refresh interval (seconds) for the project's plan. `null` =
   *  explicit refresh only. Integer ≥ 300 = background sweep cadence. */
  plan_auto_refresh_interval_secs?: number | null;
  created_at: string;
  updated_at: string;
}

export interface ProjectItem {
  skill_slug: string;
  kind: 'skill' | 'agent' | 'command';
}

export interface ProjectDetail extends Project {
  items: ProjectItem[];
}

export interface CreateProjectBody {
  slug: string;
  name: string;
  description?: string | null;
  git_remote?: string | null;
}

export interface UpdateProjectBody {
  name?: string;
  description?: string | null;
  git_remote?: string | null;
  stack_tags?: string[];
  /** Null clears the auto-refresh schedule (import only on demand). */
  plan_auto_refresh_interval_secs?: number | null;
}

export async function listProjects(auth: Auth): Promise<Project[]> {
  const resp = await call('GET', '/v1/tenant/projects', auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function createProject(
  auth: Auth,
  body: CreateProjectBody,
): Promise<{ ok: true; project: Project } | { ok: false; status: number; error: string }> {
  const resp = await call('POST', '/v1/tenant/projects', auth, { jsonBody: body });
  if (resp.ok) return { ok: true, project: (await resp.json()) as Project };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function getProject(auth: Auth, slug: string): Promise<ProjectDetail> {
  const resp = await call('GET', `/v1/tenant/projects/${encodeURIComponent(slug)}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function updateProject(
  auth: Auth,
  slug: string,
  patch: UpdateProjectBody,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('PATCH', `/v1/tenant/projects/${encodeURIComponent(slug)}`, auth, {
    jsonBody: patch,
  });
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function deleteProject(
  auth: Auth,
  slug: string,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call('DELETE', `/v1/tenant/projects/${encodeURIComponent(slug)}`, auth);
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function setProjectItems(
  auth: Auth,
  slug: string,
  items: ProjectItem[],
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call(
    'PUT',
    `/v1/tenant/projects/${encodeURIComponent(slug)}/items`,
    auth,
    { jsonBody: items },
  );
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

// --- Plugins (Layer 3) ----------------------------------------------------

/** One row in `GET /v1/plugins` — flat list shape. */
export interface Plugin {
  slug: string;
  version: string;
  name: string;
  description: string | null;
  status: 'draft' | 'published' | 'archived';
  sourcing_mode: 'internal' | 'external' | 'mirror';
  tags: string[];
  created_at: string;
}

/** A single bundled item inside a plugin (`/v1/plugins/{slug}`). */
export interface PluginContent {
  kind: 'skill' | 'agent' | 'command';
  slug: string;
  version: string;
  position: number;
}

/** Full plugin detail returned by `GET /v1/plugins/{slug}`. */
export interface PluginDetail extends Omit<Plugin, 'tags'> {
  external_git_url?: string;
  upstream_url?: string;
  manifest: Record<string, unknown>;
  contents: PluginContent[];
  updated_at: string;
}

/** Body for `POST /v1/plugins`. */
export interface PublishPluginBody {
  slug: string;
  manifest: Record<string, unknown>;
  contents: Array<{ kind: 'skill' | 'agent' | 'command'; slug: string; version: string }>;
  sourcing_mode: 'internal' | 'external' | 'mirror';
  external_git_url?: string;
  upstream_url?: string;
  status?: 'draft' | 'published';
}

export interface PluginListPage {
  items: Plugin[];
  next_cursor?: string;
}

export interface ListPluginsOptions {
  tags?: string[];
  status?: 'draft' | 'published' | 'archived';
  sourcing_mode?: 'internal' | 'external' | 'mirror';
  limit?: number;
  cursor?: string;
}

export async function listPlugins(
  auth: Auth,
  opts: ListPluginsOptions = {},
): Promise<PluginListPage> {
  const qs = new URLSearchParams();
  if (opts.tags && opts.tags.length) qs.set('tags', opts.tags.join(','));
  if (opts.status) qs.set('status', opts.status);
  if (opts.sourcing_mode) qs.set('sourcing_mode', opts.sourcing_mode);
  if (opts.limit !== undefined) qs.set('limit', String(opts.limit));
  if (opts.cursor) qs.set('cursor', opts.cursor);
  const path = qs.toString() ? `/v1/plugins?${qs.toString()}` : '/v1/plugins';
  const resp = await call('GET', path, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function getPlugin(auth: Auth, slug: string): Promise<PluginDetail> {
  const resp = await call('GET', `/v1/plugins/${encodeURIComponent(slug)}`, auth);
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

export async function publishPlugin(
  auth: Auth,
  body: PublishPluginBody,
): Promise<{ ok: true; plugin: PluginDetail } | { ok: false; status: number; error: string }> {
  const resp = await call('POST', '/v1/plugins', auth, { jsonBody: body });
  if (resp.ok) return { ok: true, plugin: (await resp.json()) as PluginDetail };
  return { ok: false, status: resp.status, error: await resp.text() };
}

export async function archivePlugin(
  auth: Auth,
  slug: string,
  version: string,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call(
    'DELETE',
    `/v1/plugins/${encodeURIComponent(slug)}/versions/${encodeURIComponent(version)}`,
    auth,
  );
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}

// --- Project Plan API -------------------------------------------------------

/** Active plan version returned by GET /v1/tenant/projects/{slug}/plan */
export interface ProjectPlan {
  version: number;
  body_md: string;
  source_type: 'file' | 'url';
  source_url: string | null;
  imported_at: string;
  imported_by_email?: string | null;
  status: 'active' | 'superseded' | 'archived';
  fetch_error?: string | null;
  fetch_error_at?: string | null;
}

/** One row in the version history list. */
export interface ProjectPlanVersion {
  version: number;
  status: 'active' | 'superseded' | 'archived';
  source_type: 'file' | 'url';
  source_url: string | null;
  imported_at: string;
  imported_by_email?: string | null;
}

/**
 * Fetch the active plan version for a project. Returns `null` when the
 * project has no plan imported yet (404 from the server).
 */
export async function getActiveProjectPlan(
  auth: Auth,
  slug: string,
): Promise<ProjectPlan | null> {
  const resp = await call(
    'GET',
    `/v1/tenant/projects/${encodeURIComponent(slug)}/plan`,
    auth,
  );
  if (resp.status === 404) return null;
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

/**
 * List all imported plan versions for a project in descending version order.
 * Returns `[]` when the project has no plan history (404) or on any error.
 */
export async function listProjectPlanVersions(
  auth: Auth,
  slug: string,
): Promise<ProjectPlanVersion[]> {
  const resp = await call(
    'GET',
    `/v1/tenant/projects/${encodeURIComponent(slug)}/plan/versions`,
    auth,
  );
  if (resp.status === 404) return [];
  if (!resp.ok) throw new ApiError(resp.status, await resp.text());
  return resp.json();
}

/**
 * Activate a specific plan version. Returns `{ ok: true }` on success or an
 * error object on failure.
 */
export async function activateProjectPlanVersion(
  auth: Auth,
  slug: string,
  version: number,
): Promise<{ ok: true } | { ok: false; status: number; error: string }> {
  const resp = await call(
    'POST',
    `/v1/tenant/projects/${encodeURIComponent(slug)}/plan/activate`,
    auth,
    { jsonBody: { version } },
  );
  if (resp.ok) return { ok: true };
  return { ok: false, status: resp.status, error: await resp.text() };
}
