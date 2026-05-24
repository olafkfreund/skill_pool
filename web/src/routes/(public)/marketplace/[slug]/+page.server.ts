import { error } from '@sveltejs/kit';
import {
  ApiError,
  getPlugin,
  listPluginVersions,
  type PluginVersionRow,
} from '$lib/server/api';
import type { PageServerLoad } from './$types';

/**
 * Build the JSON-LD SoftwareApplication structured-data object for this
 * plugin. Injected via <svelte:head> as application/ld+json for SEO.
 */
function buildJsonLd(plugin: {
  name: string;
  description: string | null;
  version: string;
  slug: string;
}, pageUrl: string): string {
  return JSON.stringify({
    '@context': 'https://schema.org',
    '@type': 'SoftwareApplication',
    name: plugin.name || plugin.slug,
    description: plugin.description ?? undefined,
    softwareVersion: plugin.version,
    applicationCategory: 'DeveloperApplication',
    operatingSystem: 'Any',
    url: pageUrl,
  });
}

export const load: PageServerLoad = async ({ locals, params, url }) => {
  // Public route: no session token. The API allows unauthenticated reads.
  const auth = { tenant: locals.tenant.slug };

  // Install command for Claude Code is /plugin marketplace add <origin>.
  const installBase = url.origin;

  try {
    const [plugin, versions] = await Promise.all([
      getPlugin(auth, params.slug),
      listPluginVersions(auth, params.slug).catch((): PluginVersionRow[] => []),
    ]);

    const pageUrl = url.href;
    const jsonLd = buildJsonLd(plugin, pageUrl);

    return {
      plugin,
      versions,
      installBase,
      jsonLd,
    };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      error(404, `Plugin "${params.slug}" not found.`);
    }
    throw e;
  }
};
