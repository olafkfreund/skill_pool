import { fail, redirect } from '@sveltejs/kit';
import { ApiError, deleteProject, listProjects } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const projects = await listProjects(auth);
    return { projects };
  } catch (e) {
    if (e instanceof ApiError) {
      return { projects: [], error: `Could not load projects: ${e.message}` };
    }
    return { projects: [], error: 'Could not load projects.' };
  }
};

export const actions: Actions = {
  delete: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const slug = String(data.get('slug') ?? '').trim();
    if (!slug) {
      return fail(400, { error: 'Project slug is required.' });
    }
    const result = await deleteProject(auth, slug);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { deleted: true, slug };
  },
};
