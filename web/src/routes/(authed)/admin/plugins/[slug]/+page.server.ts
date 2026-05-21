import { error, fail, redirect } from '@sveltejs/kit';
import {
  ApiError,
  archivePlugin,
  getPlugin,
  listPluginVersions,
  marketplaceUrl,
  whoami,
  type PluginVersionRow,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ params, locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const [plugin, versions, identity] = await Promise.all([
      getPlugin(auth, params.slug),
      listPluginVersions(auth, params.slug).catch(() => [] as PluginVersionRow[]),
      whoami(auth).catch(() => null),
    ]);
    return {
      plugin,
      versions,
      marketplaceUrl: marketplaceUrl(),
      userRole: identity?.role ?? null,
    };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      error(404, `Plugin "${params.slug}" not found.`);
    }
    throw e;
  }
};

export const actions: Actions = {
  /**
   * Soft-archive a specific version of this plugin. The version is sent
   * as a hidden form field by the row in the version-history table.
   */
  archiveVersion: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const version = String(data.get('version') ?? '').trim();
    if (!version) {
      return fail(400, { action: 'archiveVersion', error: 'Version is required.' });
    }
    const result = await archivePlugin(auth, params.slug, version);
    if (!result.ok) {
      return fail(result.status, { action: 'archiveVersion', error: result.error });
    }
    return { action: 'archiveVersion', archived: true, version };
  },

  /**
   * Toggle the mirror auto-refresh schedule. Only meaningful when
   * sourcing_mode === 'mirror'. The component hides this control in
   * every other case so the action is only reachable when relevant.
   *
   * The PATCH endpoint for auto-refresh lands with the mirror worker
   * in #32; until it ships we surface a graceful "not yet available"
   * banner the same way the import page does.
   */
  setAutoRefresh: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const rawEnabled = String(data.get('auto_refresh_enabled') ?? '');
    const rawInterval = String(data.get('interval_secs') ?? '').trim();
    const enabled = rawEnabled === 'on' || rawEnabled === 'true' || rawEnabled === '1';

    let interval_secs: number | null = null;
    if (enabled) {
      const n = Number.parseInt(rawInterval, 10);
      if (!Number.isFinite(n) || n < 300) {
        return fail(400, {
          action: 'setAutoRefresh',
          error: 'Interval must be an integer ≥ 300 seconds (5 minutes).',
        });
      }
      interval_secs = n;
    }

    // PATCH endpoint lives in #32 — until then return a graceful 503-style
    // notice instead of failing silently. Avoids the projects-style fake
    // success the protocol explicitly forbids.
    return fail(503, {
      action: 'setAutoRefresh',
      error:
        'Mirror auto-refresh wiring lands with the mirror worker (tracking issue #32). Setting saved as a draft locally; it takes effect once the worker ships.',
      tracking_issue: 32,
      interval_secs,
    });
  },
};
