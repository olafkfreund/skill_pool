import { fail } from '@sveltejs/kit';
import {
  ApiError,
  listStackMappings,
  removeStackMapping,
  upsertStackMapping,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const mappings = await listStackMappings(auth);
    return { mappings };
  } catch (e) {
    if (e instanceof ApiError) {
      return { mappings: [], error: `Could not load: ${e.message}` };
    }
    return { mappings: [], error: 'Could not load mappings.' };
  }
};

export const actions: Actions = {
  add: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const stack = String(data.get('stack') ?? '').trim();
    const skill = String(data.get('skill') ?? '').trim();
    if (!stack || !skill) {
      return fail(400, { error: 'Both stack and skill are required.' });
    }
    const result = await upsertStackMapping(auth, { stack, skill });
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { added: true, mapping: result.mapping };
  },

  remove: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const stack = String(data.get('stack') ?? '').trim();
    const skill = String(data.get('skill') ?? '').trim();
    const result = await removeStackMapping(auth, { stack, skill });
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { removed: true, mapping: { stack, skill } };
  },
};
