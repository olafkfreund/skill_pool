import { error, fail, redirect } from '@sveltejs/kit';
import { ApiError, archiveSkill, getSkillDetail, getSkillMd } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, params, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const [detail, body] = await Promise.all([
      getSkillDetail(auth, params.slug),
      getSkillMd(auth, params.slug).catch(() => ''),
    ]);
    return { detail, body };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      throw error(404, `skill "${params.slug}" not found`);
    }
    throw error(502, `registry error: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};

export const actions: Actions = {
  archive: async ({ locals, params, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const result = await archiveSkill(auth, params.slug);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    throw redirect(303, '/');
  },
};
