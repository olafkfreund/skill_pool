/**
 * Per-request hook chain.
 *
 * - Resolves the tenant from the Host header (subdomain) or `?tenant=`
 *   query string in dev. Falls back to the `SP_DEFAULT_TENANT` env var
 *   for single-host development without subdomains.
 * - Reads the `sp_token` cookie (Phase 2 auth is API-token based; OIDC
 *   takes over in #8).
 * - Stashes a TenantContext + Theme on `event.locals` so route loads see
 *   them without re-resolving.
 */

import { env } from '$env/dynamic/private';
import type { Handle } from '@sveltejs/kit';
import { DEFAULT_THEME, type Theme } from '$lib/theme';
import type { TenantContext } from '$lib/types';

function resolveTenant(url: URL, host: string | null): string {
  const override = url.searchParams.get('tenant');
  if (override) return override.toLowerCase();

  const fallback = env.SP_DEFAULT_TENANT;
  if (!host) return fallback ?? 'default';

  const hostNoPort = host.split(':')[0];
  const labels = hostNoPort.split('.');
  const first = labels[0]?.toLowerCase();

  // Drop `www.` and bare localhost/IP — fall back to env default.
  if (!first || first === 'www' || first === 'localhost' || /^\d+$/.test(first)) {
    return fallback ?? 'default';
  }
  return first;
}

function themeForTenant(_slug: string): Theme {
  // Phase 2 v1: hardcoded default. A future endpoint will return per-tenant
  // theme rows; until then we just brand by slug for visual variety.
  return DEFAULT_THEME;
}

export const handle: Handle = async ({ event, resolve }) => {
  const host = event.request.headers.get('host');
  const slug = resolveTenant(event.url, host);
  const token = event.cookies.get('sp_token') ?? undefined;
  const cookieTenant = event.cookies.get('sp_tenant');
  const authed = !!token && cookieTenant === slug;

  const tenant: TenantContext = { slug, authed };
  const theme = themeForTenant(slug);

  event.locals.tenant = tenant;
  event.locals.theme = theme;

  return resolve(event);
};
