import { error } from '@sveltejs/kit';
import { ApiError, isCatalogKind, listSkills, type CatalogKind } from '$lib/server/api';
import type { PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, url, cookies }) => {
  const q = url.searchParams.get('q') ?? '';
  // Truthy value (`'1'`, `'on'`, `'true'`) enables semantic mode.
  const semanticParam = url.searchParams.get('semantic');
  const semanticOn =
    semanticParam !== null &&
    semanticParam !== '' &&
    semanticParam !== '0' &&
    semanticParam !== 'false';

  const rawKind = url.searchParams.get('kind');
  const kind: CatalogKind = isCatalogKind(rawKind) ? rawKind : 'skill';

  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const skills = await listSkills(
      auth,
      semanticOn && q ? { semantic: q, kind } : { query: q || undefined, kind },
    );
    return { skills, query: q, semantic: semanticOn, kind };
  } catch (e) {
    if (e instanceof ApiError && e.status === 400) {
      // Surface the message so the UI can prompt the operator to enable
      // the embedder; don't 5xx the whole page.
      return { skills: [], query: q, semantic: semanticOn, kind, error: e.message };
    }
    throw error(502, `registry unreachable: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};
