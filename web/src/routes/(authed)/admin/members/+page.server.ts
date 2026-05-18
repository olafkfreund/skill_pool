import { fail } from '@sveltejs/kit';
import { ApiError, listMembers, patchMemberRole, removeMember } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const members = await listMembers(auth);
    return { members };
  } catch (e) {
    if (e instanceof ApiError) {
      return { members: [], error: `Could not load members: ${e.message}` };
    }
    return { members: [], error: 'Could not load members.' };
  }
};

const ROLES = ['viewer', 'publisher', 'curator', 'admin'] as const;
type Role = (typeof ROLES)[number];

function isRole(v: string): v is Role {
  return (ROLES as readonly string[]).includes(v);
}

export const actions: Actions = {
  setRole: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '');
    const role = String(data.get('role') ?? '');
    if (!id || !isRole(role)) {
      return fail(400, { error: 'id and a valid role are required' });
    }
    const result = await patchMemberRole(auth, id, role);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { saved: true, role: result.member.role, id };
  },

  remove: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '');
    if (!id) {
      return fail(400, { error: 'id is required' });
    }
    const result = await removeMember(auth, id);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { removed: true, id };
  },
};
