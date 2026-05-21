import { fail, redirect } from '@sveltejs/kit';
import {
  ApiError,
  archivePlugin,
  listPlugins,
  whoami,
  type ListPluginsOptions,
  type Plugin,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

/**
 * Parse the optional `?sourcing_mode=` query into the typed enum the API
 * client accepts. Anything else (missing, "all", a typo) yields `undefined`
 * so the server returns every mode.
 */
function parseSourcingMode(raw: string | null): ListPluginsOptions['sourcing_mode'] {
  if (raw === 'internal' || raw === 'external' || raw === 'mirror') return raw;
  return undefined;
}

export const load: PageServerLoad = async ({ url, locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const sourcingMode = parseSourcingMode(url.searchParams.get('sourcing_mode'));

  try {
    const [page, identity] = await Promise.all([
      listPlugins(auth, { sourcing_mode: sourcingMode }),
      whoami(auth).catch(() => null),
    ]);
    return {
      plugins: page.items,
      sourcingMode: sourcingMode ?? null,
      userRole: identity?.role ?? null,
    };
  } catch (e) {
    if (e instanceof ApiError) {
      return {
        plugins: [] as Plugin[],
        sourcingMode: sourcingMode ?? null,
        userRole: null,
        error: `Could not load plugins: ${e.message}`,
      };
    }
    return {
      plugins: [] as Plugin[],
      sourcingMode: sourcingMode ?? null,
      userRole: null,
      error: 'Could not load plugins.',
    };
  }
};

export const actions: Actions = {
  archive: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const slug = String(data.get('slug') ?? '').trim();
    const version = String(data.get('version') ?? '').trim();
    if (!slug || !version) {
      return fail(400, { error: 'Slug and version are required.' });
    }
    const result = await archivePlugin(auth, slug, version);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { archived: true, slug, version };
  },
};
