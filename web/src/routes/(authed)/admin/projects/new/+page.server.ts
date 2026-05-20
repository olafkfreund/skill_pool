import { fail, redirect } from '@sveltejs/kit';
import { createProject } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async () => {
  return {};
};

const SLUG_RE = /^[a-z0-9][a-z0-9-]*[a-z0-9]$|^[a-z0-9]$/;

export const actions: Actions = {
  default: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const slug = String(data.get('slug') ?? '').trim();
    const name = String(data.get('name') ?? '').trim();
    const description = String(data.get('description') ?? '').trim() || null;
    const git_remote = String(data.get('git_remote') ?? '').trim() || null;

    if (!slug) {
      return fail(400, { error: 'Slug is required.', slug, name, description, git_remote });
    }
    if (!SLUG_RE.test(slug)) {
      return fail(400, {
        error: 'Slug may only contain lowercase letters, digits, and hyphens.',
        slug,
        name,
        description,
        git_remote,
      });
    }
    if (!name) {
      return fail(400, { error: 'Name is required.', slug, name, description, git_remote });
    }
    if (git_remote) {
      try {
        new URL(git_remote);
      } catch {
        return fail(400, {
          error: 'Git remote must be a valid URL.',
          slug,
          name,
          description,
          git_remote,
        });
      }
    }

    const result = await createProject(auth, { slug, name, description, git_remote });
    if (!result.ok) {
      return fail(result.status, {
        error: result.error,
        slug,
        name,
        description,
        git_remote,
      });
    }

    redirect(303, `/admin/projects/${encodeURIComponent(result.project.slug)}`);
  },
};
