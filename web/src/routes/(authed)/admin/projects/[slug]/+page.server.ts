import { error, fail, redirect } from '@sveltejs/kit';
import {
  ApiError,
  deleteProject,
  getProject,
  setProjectItems,
  updateProject,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';
import type { ProjectItem } from '$lib/server/api';

export const load: PageServerLoad = async ({ params, locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const project = await getProject(auth, params.slug);
    return { project };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      error(404, `Project "${params.slug}" not found.`);
    }
    throw e;
  }
};

export const actions: Actions = {
  /** Update metadata: name, description, git_remote. */
  updateMeta: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const name = String(data.get('name') ?? '').trim();
    const description = String(data.get('description') ?? '').trim() || null;
    const git_remote = String(data.get('git_remote') ?? '').trim() || null;

    if (!name) {
      return fail(400, { action: 'meta', error: 'Name is required.' });
    }
    if (git_remote) {
      try {
        new URL(git_remote);
      } catch {
        return fail(400, { action: 'meta', error: 'Git remote must be a valid URL.' });
      }
    }

    const result = await updateProject(auth, params.slug, { name, description, git_remote });
    if (!result.ok) {
      return fail(result.status, { action: 'meta', error: result.error });
    }
    return { action: 'meta', saved: true };
  },

  /** Replace the stack_tags list. */
  setTags: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const raw = String(data.get('stack_tags') ?? '');
    const stack_tags = raw
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean);

    const result = await updateProject(auth, params.slug, { stack_tags });
    if (!result.ok) {
      return fail(result.status, { action: 'tags', error: result.error });
    }
    return { action: 'tags', saved: true };
  },

  /** Add a single item (skill/agent/command) to the project. */
  addItem: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const skill_slug = String(data.get('skill_slug') ?? '').trim();
    const kind = String(data.get('kind') ?? '') as ProjectItem['kind'];

    if (!skill_slug) {
      return fail(400, { action: 'addItem', kind, error: 'Slug is required.' });
    }
    if (!['skill', 'agent', 'command'].includes(kind)) {
      return fail(400, { action: 'addItem', kind, error: 'Invalid kind.' });
    }

    // Load current items, append the new one (deduplicated), then set.
    let current: ProjectItem[] = [];
    try {
      const proj = await getProject(auth, params.slug);
      current = proj.items;
    } catch {
      return fail(500, { action: 'addItem', kind, error: 'Could not load current items.' });
    }

    const alreadyExists = current.some(
      (it) => it.skill_slug === skill_slug && it.kind === kind,
    );
    if (alreadyExists) {
      return fail(409, {
        action: 'addItem',
        kind,
        error: `${kind} "${skill_slug}" is already in this project.`,
      });
    }

    const next: ProjectItem[] = [...current, { skill_slug, kind }];
    const result = await setProjectItems(auth, params.slug, next);
    if (!result.ok) {
      return fail(result.status, { action: 'addItem', kind, error: result.error });
    }
    return { action: 'addItem', kind, added: true, skill_slug };
  },

  /** Remove a single item from the project. */
  removeItem: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const skill_slug = String(data.get('skill_slug') ?? '').trim();
    const kind = String(data.get('kind') ?? '') as ProjectItem['kind'];

    if (!skill_slug || !kind) {
      return fail(400, { action: 'removeItem', kind, error: 'skill_slug and kind are required.' });
    }

    let current: ProjectItem[] = [];
    try {
      const proj = await getProject(auth, params.slug);
      current = proj.items;
    } catch {
      return fail(500, { action: 'removeItem', kind, error: 'Could not load current items.' });
    }

    const next = current.filter(
      (it) => !(it.skill_slug === skill_slug && it.kind === kind),
    );
    const result = await setProjectItems(auth, params.slug, next);
    if (!result.ok) {
      return fail(result.status, { action: 'removeItem', kind, error: result.error });
    }
    return { action: 'removeItem', kind, removed: true, skill_slug };
  },

  /** Delete the entire project and redirect to the listing. */
  deleteProject: async ({ params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const result = await deleteProject(auth, params.slug);
    if (!result.ok) {
      return fail(result.status, { action: 'deleteProject', error: result.error });
    }
    redirect(303, '/admin/projects');
  },
};
