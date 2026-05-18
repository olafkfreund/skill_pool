import { error, fail, redirect } from '@sveltejs/kit';
import { ApiError, getDraft, getDraftSkillMd, patchDraft } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ params, locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  try {
    const draft = await getDraft(auth, params.id);
    // Best-effort: fetch the rendered SKILL.md too so curators can read
    // the body context while editing. Failure here is non-fatal — we
    // still render the edit form.
    let skillMd: string | null = null;
    try {
      skillMd = await getDraftSkillMd(auth, params.id);
    } catch {
      // bundle missing on disk or transient blip — show without it
    }
    return { draft, skillMd };
  } catch (e) {
    if (e instanceof ApiError && e.status === 404) {
      throw error(404, 'Draft not found.');
    }
    throw error(502, `registry unreachable: ${e instanceof Error ? e.message : 'unknown'}`);
  }
};

function parseTags(raw: string): string[] {
  return raw
    .split(',')
    .map((t) => t.trim())
    .filter((t) => t.length > 0);
}

export const actions: Actions = {
  save: async ({ request, params, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const result = await patchDraft(auth, params.id, {
      slug: String(data.get('slug') ?? '').trim(),
      description: String(data.get('description') ?? '').trim(),
      when_to_use: String(data.get('when_to_use') ?? ''),
      tags: parseTags(String(data.get('tags') ?? '')),
      notes: String(data.get('notes') ?? ''),
    });
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    throw redirect(303, '/drafts');
  },
};
