/**
 * Action-level tests for the plugins composer.
 *
 * Lives in its own file because these specs `vi.mock('$lib/server/api')`
 * to stub `publishPlugin` and `whoami`. The component-render tests in
 * `plugins.test.ts` deliberately avoid mocks (they exercise rendered
 * markup against fixture props) — mixing the two in one file would make
 * the mock state leak across describe blocks.
 *
 * The redirect spec is the team-lead-required mod from the plan review:
 * confirm that on a 201 from `publishPlugin`, the composer action throws
 * SvelteKit's `Redirect` to `/admin/plugins/<slug>` so the curator lands
 * on the detail page instead of staring at an empty form.
 */

import { afterEach, describe, expect, it, vi } from 'vitest';
import { isRedirect } from '@sveltejs/kit';

// Mock the api module before importing the action so the import-time
// references resolve to the stubs. `whoami` is called twice in the
// action (load gate + action gate); both need to return a curator.
vi.mock('$lib/server/api', async (orig) => {
  const actual = await orig<typeof import('$lib/server/api')>();
  return {
    ...actual,
    whoami: vi.fn(async () => ({
      user_id: 'u1',
      email: 'curator@acme.com',
      role: 'curator',
      tenant: 'acme',
    })),
    listSkills: vi.fn(async () => []),
    publishPlugin: vi.fn(),
  };
});

// $env/dynamic/private is shimmed by SvelteKit at test time; the api
// module reads `SKILL_POOL_API_BASE` lazily, so as long as we mock
// `publishPlugin` we never hit the real HTTP call path.

afterEach(() => {
  vi.clearAllMocks();
});

/** Build a minimal RequestEvent the action can read from. */
function buildEvent(formFields: Record<string, string>) {
  const form = new FormData();
  for (const [k, v] of Object.entries(formFields)) form.append(k, v);
  return {
    request: new Request('https://example.test/admin/plugins/new', {
      method: 'POST',
      body: form,
    }),
    locals: { tenant: { slug: 'acme' } },
    cookies: { get: (_: string) => 'fake-token' },
    params: {},
    url: new URL('https://example.test/admin/plugins/new'),
  } as unknown as Parameters<
    NonNullable<
      Awaited<typeof import('../src/routes/(authed)/admin/plugins/new/+page.server')>['actions']
    >['default']
  >[0];
}

describe('admin/plugins/new — action success path', () => {
  it('redirects to /admin/plugins/<slug> on a 201 from publishPlugin', async () => {
    const api = await import('$lib/server/api');
    const mod = await import('../src/routes/(authed)/admin/plugins/new/+page.server');

    // Stub publishPlugin → 201 with a freshly-minted PluginDetail.
    vi.mocked(api.publishPlugin).mockResolvedValueOnce({
      ok: true,
      plugin: {
        slug: 'rust-axum-toolkit',
        version: '1.2.0',
        name: 'Rust + Axum Toolkit',
        description: 'Curated skills',
        status: 'published',
        sourcing_mode: 'internal',
        manifest: {},
        contents: [],
        created_at: '2026-05-21T00:00:00Z',
        updated_at: '2026-05-21T00:00:00Z',
      } as any,
    });

    const event = buildEvent({
      slug: 'rust-axum-toolkit',
      displayName: 'Rust + Axum Toolkit',
      version: '1.2.0',
      description: 'Curated skills',
      sourcing_mode: 'internal',
      external_git_url: '',
      selected_skills: 'rust-error-handling@1.0.0',
      selected_agents: '',
      selected_commands: '',
      hooks_json: '',
      mcp_servers_json: '',
      lsp_servers_json: '',
      monitors_json: '',
    });

    // Action throws SvelteKit's Redirect on success. Catch + assert.
    let thrown: unknown = null;
    try {
      await (mod.actions as any).default(event);
    } catch (e) {
      thrown = e;
    }

    expect(thrown, 'action should throw a Redirect on success').not.toBeNull();
    expect(isRedirect(thrown), 'thrown value should be a SvelteKit Redirect').toBe(true);
    const r = thrown as { status: number; location: string };
    expect(r.status).toBe(303);
    expect(r.location).toBe('/admin/plugins/rust-axum-toolkit');
    expect(api.publishPlugin).toHaveBeenCalledTimes(1);
  });
});
