import { redirect } from '@sveltejs/kit';
import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals, url }) => {
  if (!locals.tenant.authed) {
    const next = url.pathname + url.search;
    throw redirect(303, `/login?next=${encodeURIComponent(next)}`);
  }
  return {};
};
