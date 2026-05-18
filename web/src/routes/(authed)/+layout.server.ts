import { redirect } from '@sveltejs/kit';
import { pendingDraftsCount } from '$lib/server/api';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals, url, cookies }) => {
  if (!locals.tenant.authed) {
    const next = url.pathname + url.search;
    throw redirect(303, `/login?next=${encodeURIComponent(next)}`);
  }
  // Best-effort: a flaky count shouldn't break the navigation chrome.
  let pendingDrafts = 0;
  try {
    pendingDrafts = await pendingDraftsCount({
      tenant: locals.tenant.slug,
      token: cookies.get('sp_token'),
    });
  } catch {
    // swallow — render the sidebar with no badge
  }
  return { pendingDrafts };
};
