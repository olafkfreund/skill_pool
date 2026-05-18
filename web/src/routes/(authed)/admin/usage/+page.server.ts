import { ApiError, getUsageTimeline, getUsageTop } from '$lib/server/api';
import type { PageServerLoad } from './$types';

const DEFAULT_DAYS = 30;

function parseDays(v: string | null): number {
  if (v === null) return DEFAULT_DAYS;
  const n = Number.parseInt(v, 10);
  if (!Number.isFinite(n)) return DEFAULT_DAYS;
  return Math.min(Math.max(n, 1), 365);
}

export const load: PageServerLoad = async ({ locals, cookies, url }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const days = parseDays(url.searchParams.get('days'));
  try {
    const [timeline, top] = await Promise.all([
      getUsageTimeline(auth, days),
      getUsageTop(auth, days, 10),
    ]);
    return { timeline, top, days };
  } catch (e) {
    const error =
      e instanceof ApiError ? `Could not load usage: ${e.message}` : 'Could not load usage.';
    return { timeline: [], top: [], days, error };
  }
};
