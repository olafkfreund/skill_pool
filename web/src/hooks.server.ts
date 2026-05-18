import { env } from '$env/dynamic/private';
import type { Handle } from '@sveltejs/kit';
import { DEFAULT_THEME, type Theme } from '$lib/theme';
import type { TenantContext } from '$lib/types';
import { getTheme, toClientTheme } from '$lib/server/api';

function resolveTenant(url: URL, host: string | null): string {
  const override = url.searchParams.get('tenant');
  if (override) return override.toLowerCase();

  const fallback = env.SP_DEFAULT_TENANT;
  if (!host) return fallback ?? 'default';

  const hostNoPort = host.split(':')[0];
  const first = hostNoPort.split('.')[0]?.toLowerCase();

  if (!first || first === 'www' || first === 'localhost' || /^\d+$/.test(first)) {
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

  event.locals.tenant = tenant;
  event.locals.theme = theme;

  return resolve(event);
};
