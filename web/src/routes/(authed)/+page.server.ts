import { error } from '@sveltejs/kit';
import { listSkills } from '$lib/server/api';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, url, cookies }) => {
  const q = url.searchParams.get('q') ?? '';
  try {
    const skills = await listSkills(
      { tenant: locals.tenant.slug, token: cookies.get('sp_token') },
      q || undefined,
    );
    return { skills, query: q };
  } catch (e) {
    throw error(502, `registry unreachable: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};
