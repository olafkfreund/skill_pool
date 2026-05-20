import { error, fail, redirect } from '@sveltejs/kit';
import {
  ApiError,
  activateProjectPlanVersion,
  deleteProject,
  getActiveProjectPlan,
  getProject,
  listProjectPlanVersions,
  setProjectItems,
  updateProject,
  whoami,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';
import type { ProjectItem } from '$lib/server/api';

export const load: PageServerLoad = async ({ params, locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const [project, plan, planVersions, identity] = await Promise.all([
      getProject(auth, params.slug),
      getActiveProjectPlan(auth, params.slug).catch(() => null),
      listProjectPlanVersions(auth, params.slug).catch(() => []),
      whoami(auth).catch(() => null),
    ]);
    return { project, plan, planVersions, userRole: identity?.role ?? null };
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

  /**
   * Activate a specific plan version. The version number comes from the form
   * as a hidden field submitted by the "Activate" button in the version
   * history table.
   */
  activatePlanVersion: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const rawVersion = String(data.get('version') ?? '').trim();
    const version = Number.parseInt(rawVersion, 10);
    if (!Number.isFinite(version) || version < 1) {
      return fail(400, { action: 'activatePlanVersion', error: 'Invalid version number.' });
    }
    const result = await activateProjectPlanVersion(auth, params.slug, version);
    if (!result.ok) {
      return fail(result.status, { action: 'activatePlanVersion', error: result.error });
    }
    return { action: 'activatePlanVersion', activated: true, version };
  },

  /**
   * Save the plan auto-refresh interval.  An empty / zero value disables
   * auto-refresh (PATCH with null).  The minimum accepted value is 300 s.
   */
  setAutoRefresh: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const rawEnabled = String(data.get('auto_refresh_enabled') ?? '');
    const rawInterval = String(data.get('interval_secs') ?? '').trim();

    const enabled = rawEnabled === 'on' || rawEnabled === 'true' || rawEnabled === '1';

    let interval_secs: number | null = null;
    if (enabled) {
      const n = Number.parseInt(rawInterval, 10);
      if (!Number.isFinite(n) || n < 300) {
        return fail(400, {
          action: 'setAutoRefresh',
          error: 'Interval must be an integer ≥ 300 seconds (5 minutes).',
        });
      }
      interval_secs = n;
    }

    const result = await updateProject(auth, params.slug, {
      plan_auto_refresh_interval_secs: interval_secs,
    });
    if (!result.ok) {
      return fail(result.status, { action: 'setAutoRefresh', error: result.error });
    }
    return { action: 'setAutoRefresh', saved: true, interval_secs };
  },
};
