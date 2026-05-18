import { fail } from '@sveltejs/kit';
import { ApiError, listDrafts, publishDraft, discardDraft } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

type StatusFilter = 'pending' | 'published' | 'discarded' | 'all';

function asStatus(v: string | null): StatusFilter {
  return v === 'published' || v === 'discarded' || v === 'all' ? v : 'pending';
}

export const load: PageServerLoad = async ({ locals, cookies, url }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const status = asStatus(url.searchParams.get('status'));
  try {
    const drafts = await listDrafts(auth, status);
    return { drafts, status };
  } catch (e) {
    if (e instanceof ApiError) {
      return { drafts: [], status, error: `Could not load drafts: ${e.message}` };
    }
    return { drafts: [], status, error: 'Could not load drafts.' };
  }
};

export const actions: Actions = {
  publish: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '');
    const version = String(data.get('version') ?? '').trim();
    const slugRaw = String(data.get('slug') ?? '').trim();
    if (!id || !version) {
      return fail(400, { error: 'id and version are required' });
    }
    const result = await publishDraft(auth, id, version, slugRaw || undefined);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { published: true, id, slug: result.slug, version: result.version };
  },

  discard: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '');
    if (!id) {
      return fail(400, { error: 'id is required' });
    }
    const result = await discardDraft(auth, id);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { discarded: true, id };
  },
};
