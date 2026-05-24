import { ApiError, listPlugins, type Plugin } from '$lib/server/api';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, url }) => {
  // Public route: no session token. The API allows unauthenticated reads
  // on GET /v1/plugins (see docs/plugins.md authorization table).
  const auth = { tenant: locals.tenant.slug };

  // The install command for Claude Code is:
  //   /plugin marketplace add <origin>
  // where <origin> is the tenant's base URL (e.g. https://acme.example.com).
  // We derive this from the request URL so it is accurate for every tenant
  // subdomain without hard-coding the registry host.
  const installBase = url.origin;

  try {
    const page = await listPlugins(auth, { status: 'published' });
    return {
      plugins: page.items,
      installBase,
    };
  } catch (e) {
    if (e instanceof ApiError) {
      return {
        plugins: [] as Plugin[],
        installBase,
        error: `Could not load plugins: ${e.message}`,
      };
    }
    return {
      plugins: [] as Plugin[],
      installBase,
      error: 'Could not load plugins.',
    };
  }
};
