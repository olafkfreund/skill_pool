import { env } from '$env/dynamic/private';
import type { Handle } from '@sveltejs/kit';
import { DEFAULT_THEME, type Theme } from '$lib/theme';
import type { TenantContext } from '$lib/types';
import { getTheme, toClientTheme } from '$lib/server/api';

const DEFAULT_API_BASE = 'http://127.0.0.1:8080';

/**
 * Cheap HEAD-like probe to determine whether the tenant has a custom-CSS
 * overlay set. We can't use HEAD (the server doesn't expose it) so we issue
 * a GET and immediately discard the body — the overlay is at most 32 KiB so
 * the wasted bandwidth is negligible compared to the alternative (an extra
 * column in `GET /v1/theme` just for a boolean flag).
 */
async function hasCustomCssFor(slug: string): Promise<boolean> {
  const apiBase = env.SKILL_POOL_API_BASE?.replace(/\/$/, '') ?? DEFAULT_API_BASE;
  try {
    const resp = await fetch(`${apiBase}/v1/theme/custom.css`, {
      method: 'GET',
      headers: { 'X-Skill-Pool-Tenant': slug },
    });
    return resp.ok;
  } catch {
    return false;
  }
}

function resolveTenant(url: URL, host: string | null): string {
  const override = url.searchParams.get('tenant');
  if (override) return override.toLowerCase();

  const fallback = env.SP_DEFAULT_TENANT;
  if (!host) return fallback ?? 'default';

  const hostNoPort = host.split(':')[0].toLowerCase();
  const first = hostNoPort.split('.')[0];

  // Skiplist: hostnames where the first label is not a tenant subdomain
  // but a LAN/dev identifier. Fall back to SP_DEFAULT_TENANT instead of
  // treating it as a real tenant slug.
  //   - localhost / www / numeric IPs: classic loopback / generic
  //   - *.lan / *.local: mDNS/Avahi LAN-local domains (e.g. razer.lan)
  //   - *.nip.io: the free wildcard DNS we use for AWS deploys
  if (
    !first ||
    first === 'www' ||
    first === 'localhost' ||
    /^\d+$/.test(first) ||
    hostNoPort.endsWith('.lan') ||
    hostNoPort.endsWith('.local') ||
    hostNoPort.endsWith('.nip.io') ||
    hostNoPort.includes('.') === false
  ) {
    return fallback ?? 'default';
  }
  return first;
}

async function themeFor(slug: string): Promise<Theme> {
  try {
    const serverTheme = await getTheme({ tenant: slug });
    if (serverTheme) return toClientTheme(serverTheme);
  } catch {
    // Fall through to default — surface "API unreachable" via the catalog page
    // rather than crashing on every request.
  }
  return { ...DEFAULT_THEME, brandName: slug };
}

export const handle: Handle = async ({ event, resolve }) => {
  const host = event.request.headers.get('host');
  const slug = resolveTenant(event.url, host);
  const token = event.cookies.get('sp_token') ?? undefined;
  const cookieTenant = event.cookies.get('sp_tenant');
  const authed = !!token && cookieTenant === slug;

  const tenant: TenantContext = { slug, authed };
  const theme = await themeFor(slug);
  const hasCustomCss = await hasCustomCssFor(slug);

  event.locals.tenant = tenant;
  event.locals.theme = theme;
  event.locals.hasCustomCss = hasCustomCss;

  return resolve(event);
};
