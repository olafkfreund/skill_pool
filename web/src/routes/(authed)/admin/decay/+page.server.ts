import { fail } from '@sveltejs/kit';
import { ApiError, archiveSkill, listDecayCandidates } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

const DEFAULT_DAYS = 180;
const DEFAULT_MAX_USES = 3;

function parseIntParam(v: string | null, fallback: number, min: number, max: number): number {
  if (v === null) return fallback;
  const n = Number.parseInt(v, 10);
  if (!Number.isFinite(n)) return fallback;
  return Math.min(Math.max(n, min), max);
}

export const load: PageServerLoad = async ({ locals, cookies, url }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const days = parseIntParam(url.searchParams.get('days'), DEFAULT_DAYS, 1, 365 * 5);
  const maxUses = parseIntParam(url.searchParams.get('max_uses'), DEFAULT_MAX_USES, 0, 100);
  try {
    const candidates = await listDecayCandidates(auth, { days, maxUses, limit: 200 });
    return { candidates, days, maxUses };
  } catch (e) {
    const error =
      e instanceof ApiError ? `Could not load: ${e.message}` : 'Could not load candidates.';
    return { candidates: [], days, maxUses, error };
  }
};

export const actions: Actions = {
  archive: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const slug = String(data.get('slug') ?? '').trim();
    if (!slug) {
      return fail(400, { error: 'slug is required' });
    }
    const result = await archiveSkill(auth, slug);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { archived: true, slug: result.slug, version: result.version };
  },
};
