import { fail, redirect } from '@sveltejs/kit';
import { buildSkillBundle } from '$lib/server/tar';
import { publishSkill, validateSkill } from '$lib/server/api';
import type { Actions } from './$types';

const TEMPLATE = `---
name: my-new-skill
description: Describe what this skill is for and when Claude should invoke it.
when_to_use: User explicitly asks about X, or is doing Y and would benefit from Z.
tags: [example]
---

# my-new-skill

Body of the skill. Write the instructions Claude will read when this skill loads.

## Example

\`\`\`bash
# Show a concrete example here.
\`\`\`
`;

export const load = async () => {
  return { template: TEMPLATE };
};

interface Draft {
  slug: string;
  version: string;
  tags: string;
  skillMd: string;
}

function readDraft(form: FormData): Draft {
  return {
    slug: String(form.get('slug') ?? '').trim(),
    version: String(form.get('version') ?? '').trim(),
    tags: String(form.get('tags') ?? '').trim(),
    skillMd: String(form.get('skillMd') ?? ''),
  };
}

function parseTags(raw: string): string[] {
  return raw
    .split(',')
    .map((t) => t.trim())
    .filter((t) => t.length > 0);
}

export const actions: Actions = {
  validate: async ({ request, locals, cookies }) => {
    const draft = readDraft(await request.formData());
    if (!draft.skillMd.trim()) {
      return fail(400, { draft, error: 'SKILL.md body is empty' });
    }

    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const bundle = buildSkillBundle(draft.skillMd);
    const result = await validateSkill(auth, bundle);

    if (!result.ok) {
      return fail(400, { draft, error: result.error ?? 'validation failed' });
    }
    return {
      draft,
      validated: {
        name: result.name,
        description: result.description,
        tags: result.tags ?? [],
      },
    };
  },

  publish: async ({ request, locals, cookies }) => {
    const draft = readDraft(await request.formData());

    if (!draft.slug) return fail(400, { draft, error: '`slug` is required' });
    if (!draft.version) return fail(400, { draft, error: '`version` is required (e.g. 1.0.0)' });
    if (!draft.skillMd.trim()) return fail(400, { draft, error: 'SKILL.md body is empty' });

    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const bundle = buildSkillBundle(draft.skillMd);
    const result = await publishSkill(
      auth,
      { slug: draft.slug, version: draft.version, tags: parseTags(draft.tags) },
      bundle,
    );

    if (!result.ok) {
      return fail(result.status, { draft, error: result.error });
    }

    throw redirect(303, `/skills/${encodeURIComponent(result.skill.slug)}`);
  },
};
