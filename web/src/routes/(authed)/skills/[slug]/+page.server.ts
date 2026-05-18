import { error } from '@sveltejs/kit';
import { ApiError, getSkill } from '$lib/server/api';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, params, cookies }) => {
  try {
    const skill = await getSkill(
      { tenant: locals.tenant.slug, token: cookies.get('sp_token') },
      params.slug,
    );
    return { skill };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      throw error(404, `skill "${params.slug}" not found`);
    }
    throw error(502, `registry error: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};
