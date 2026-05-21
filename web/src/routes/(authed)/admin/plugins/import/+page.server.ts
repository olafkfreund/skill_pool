import { error, fail, redirect } from '@sveltejs/kit';
import { importPlugin, whoami } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

function requireCurator(role: string | null): asserts role is 'curator' | 'admin' {
  if (role !== 'curator' && role !== 'admin') {
    error(403, 'Curator role required to import plugins.');
  }
}

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const identity = await whoami(auth).catch(() => null);
  const userRole = identity?.role ?? null;
  requireCurator(userRole);
  return { userRole };
};

export const actions: Actions = {
  default: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const identity = await whoami(auth).catch(() => null);
    requireCurator(identity?.role ?? null);

    const data = await request.formData();
    const url = String(data.get('url') ?? '').trim();
    const rawInterval = String(data.get('refresh_interval_secs') ?? '').trim();

    if (!url) {
      return fail(400, { url, refresh_interval_secs: rawInterval, error: 'Git URL is required.' });
    }
    try {
      new URL(url);
    } catch {
      return fail(400, {
        url,
        refresh_interval_secs: rawInterval,
        error: 'Git URL must be a valid URL.',
      });
    }

    let refresh_interval_secs: number | undefined;
    if (rawInterval) {
      const n = Number.parseInt(rawInterval, 10);
      if (!Number.isFinite(n) || n < 300) {
        return fail(400, {
          url,
          refresh_interval_secs: rawInterval,
          error: 'Refresh interval must be an integer ≥ 300 seconds (5 minutes).',
        });
      }
      refresh_interval_secs = n;
    }

    const result = await importPlugin(auth, url, { refresh_interval_secs });

    if (result.ok) {
      return { imported: true, job_id: result.job_id, url };
    }

    if (result.notYetAvailable) {
      // Graceful 503: the page renders the dedicated tracking-issue banner
      // rather than a generic error toast. Mirrors the CLI's exit-2
      // behaviour ("the surface exists but the worker doesn't").
      return fail(503, {
        url,
        refresh_interval_secs: rawInterval,
        error:
          'Plugin import is not yet available — the async import worker lands in tracking issue #32.',
        notYetAvailable: true,
        tracking_issue: 32,
      });
    }

    return fail(result.status, { url, refresh_interval_secs: rawInterval, error: result.error });
  },
};
