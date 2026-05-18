import { error } from '@sveltejs/kit';
import { ApiError, getSkill, getSkillMd } from '$lib/server/api';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, params, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const [skill, body] = await Promise.all([
      getSkill(auth, params.slug),
      getSkillMd(auth, params.slug).catch(() => ''),
    ]);
    return { skill, body };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      throw error(404, `skill "${params.slug}" not found`);
    }
    throw error(502, `registry error: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};
