import { error, fail, redirect } from '@sveltejs/kit';
import {
  ApiError,
  archiveSkill,
  getSkillDetail,
  getSkillMd,
  isCatalogKind,
  type CatalogKind,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

function resolveKind(url: URL): CatalogKind {
  const v = url.searchParams.get('kind');
  return isCatalogKind(v) ? v : 'skill';
}

export const load: PageServerLoad = async ({ locals, params, cookies, url }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const kind = resolveKind(url);
  try {
    const [detail, body] = await Promise.all([
      getSkillDetail(auth, params.slug, kind),
      getSkillMd(auth, params.slug, kind).catch(() => ''),
    ]);
    return { detail, body, kind };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      throw error(404, `${kind} "${params.slug}" not found`);
    }
    throw error(502, `registry error: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};

export const actions: Actions = {
  archive: async ({ locals, params, cookies, url }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const kind = resolveKind(url);
    const result = await archiveSkill(auth, params.slug, kind);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    const params2 = new URLSearchParams();
    if (kind !== 'skill') params2.set('kind', kind);
    throw redirect(303, params2.size ? `/?${params2}` : '/');
  },
};
