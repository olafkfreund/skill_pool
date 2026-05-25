import { fail, redirect } from '@sveltejs/kit';
import { buildSkillBundle } from '$lib/server/tar';
import { isCatalogKind, publishSkill, validateSkill, type CatalogKind } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

// One starter template per kind. Body is intentionally minimal —
// curators replace it; the frontmatter shape is what the server cares
// about.
const TEMPLATES: Record<CatalogKind, string> = {
  skill: `---
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
`,
  agent: `---
name: my-new-agent
description: A focused subagent that handles <task>. Used by the parent Claude session.
when_to_use: Delegate when the user is doing <thing> and benefits from a fresh context window.
tags: [example, agent]
---

# my-new-agent

You are a specialised assistant that <persona>. Be terse, factual, and only
do <task>. Don't speculate beyond what you can verify from the inputs.

## Capabilities

- <capability 1>
- <capability 2>

## Boundaries

- <what you must not do>
`,
  command: `---
name: my-new-command
description: A slash-command that codifies a repeatable workflow.
when_to_use: User types /my-new-command. Should be invocable without any further context.
tags: [example, command]
---

# my-new-command

Run the following steps in order:

1. <step 1>
2. <step 2>
3. <step 3>

Stop and report when any step fails.
`,
};

export const load: PageServerLoad = async ({ url }) => {
  const rawKind = url.searchParams.get('kind');
  const kind: CatalogKind = isCatalogKind(rawKind) ? rawKind : 'skill';
  return { template: TEMPLATES[kind], kind };
};

interface Draft {
  slug: string;
  version: string;
  tags: string;
  skillMd: string;
  kind: CatalogKind;
}

function readDraft(form: FormData): Draft {
  const rawKind = String(form.get('kind') ?? '').trim();
  return {
    slug: String(form.get('slug') ?? '').trim(),
    version: String(form.get('version') ?? '').trim(),
    tags: String(form.get('tags') ?? '').trim(),
    skillMd: String(form.get('skillMd') ?? ''),
    kind: isCatalogKind(rawKind) ? rawKind : 'skill',
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
      {
        slug: draft.slug,
        version: draft.version,
        tags: parseTags(draft.tags),
        kind: draft.kind,
      },
      bundle,
    );

    if (!result.ok) {
      return fail(result.status, { draft, error: result.error });
    }

    const detailQuery = draft.kind === 'skill' ? '' : `?kind=${draft.kind}`;
    throw redirect(303, `/skills/${encodeURIComponent(result.skill.slug)}${detailQuery}`);
  },
};
