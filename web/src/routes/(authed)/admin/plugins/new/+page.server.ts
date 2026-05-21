import { error, fail, redirect } from '@sveltejs/kit';
import {
  ApiError,
  listSkills,
  publishPlugin,
  whoami,
  type CatalogKind,
  type PublishPluginBody,
} from '$lib/server/api';
import type { Skill } from '$lib/types';
import type { Actions, PageServerLoad } from './$types';

const SLUG_RE = /^[a-z0-9]+(-[a-z0-9]+)*$/;
const SEMVER_RE = /^\d+\.\d+\.\d+(?:[-+].+)?$/;
const MAX_CONTENTS = 64;

/**
 * Server-side curator gate. Mirrors the visibility logic in the list
 * page's "+ New plugin" button — deep-linking is rejected with 403 so a
 * hidden button can't be bypassed.
 */
function requireCurator(role: string | null): asserts role is 'curator' | 'admin' {
  if (role !== 'curator' && role !== 'admin') {
    error(403, 'Curator role required to compose a plugin.');
  }
}

/**
 * Fetch up to 500 entries per kind so the composer's type-ahead has
 * something to filter against. 500 matches the issue's stated scaling
 * target ("when a tenant has 500+ skills, a vanilla <select multiple>
 * is unusable") — beyond that the list-server route paginates and the
 * composer surfaces a hint to use the search box.
 */
async function loadCatalog(
  auth: { tenant: string; token?: string },
  kind: CatalogKind,
): Promise<Skill[]> {
  try {
    return await listSkills(auth, { kind, limit: 500 });
  } catch {
    return [];
  }
}

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const identity = await whoami(auth).catch(() => null);
  const userRole = identity?.role ?? null;
  requireCurator(userRole);

  const [skills, agents, commands] = await Promise.all([
    loadCatalog(auth, 'skill'),
    loadCatalog(auth, 'agent'),
    loadCatalog(auth, 'command'),
  ]);

  return { skills, agents, commands, userRole };
};

/**
 * Parse an optional inline JSON blob from a textarea. Empty/whitespace
 * input yields `undefined` so the manifest stays compact. Parse failure
 * raises a typed error with the section name so the action can surface
 * "<section> JSON: <message>".
 */
function parseInlineJson(
  raw: string,
  section: 'hooks' | 'mcpServers' | 'lspServers' | 'monitors',
): { ok: true; value: unknown | undefined } | { ok: false; error: string; section: string } {
  const trimmed = raw.trim();
  if (!trimmed) return { ok: true, value: undefined };
  try {
    return { ok: true, value: JSON.parse(trimmed) };
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    return { ok: false, error: `${section} JSON: ${msg}`, section };
  }
}

/**
 * Parse the comma-separated list of slug@version pairs that the
 * composer submits for one kind. Whitespace, blank entries, and
 * trailing commas are tolerated. Returns an empty array when the
 * input is empty.
 */
function parseSelected(
  raw: string,
  kind: 'skill' | 'agent' | 'command',
): Array<{ kind: 'skill' | 'agent' | 'command'; slug: string; version: string }> {
  return raw
    .split(',')
    .map((entry) => entry.trim())
    .filter(Boolean)
    .map((entry) => {
      const [slug, version] = entry.split('@', 2);
      return { kind, slug: slug?.trim() ?? '', version: (version ?? '').trim() };
    })
    .filter((it) => it.slug.length > 0 && it.version.length > 0);
}

export const actions: Actions = {
  default: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const identity = await whoami(auth).catch(() => null);
    requireCurator(identity?.role ?? null);

    const data = await request.formData();

    const slug = String(data.get('slug') ?? '').trim();
    const displayName = String(data.get('displayName') ?? '').trim();
    const version = String(data.get('version') ?? '').trim();
    const description = String(data.get('description') ?? '').trim();
    const sourcing_mode = String(data.get('sourcing_mode') ?? 'internal') as
      | 'internal'
      | 'external'
      | 'mirror';
    const external_git_url = String(data.get('external_git_url') ?? '').trim();

    const rawSkills = String(data.get('selected_skills') ?? '');
    const rawAgents = String(data.get('selected_agents') ?? '');
    const rawCommands = String(data.get('selected_commands') ?? '');

    const hooksRaw = String(data.get('hooks_json') ?? '');
    const mcpServersRaw = String(data.get('mcp_servers_json') ?? '');
    const lspServersRaw = String(data.get('lsp_servers_json') ?? '');
    const monitorsRaw = String(data.get('monitors_json') ?? '');

    // Echoed back on every failure so the user doesn't lose their work.
    const echo = {
      slug,
      displayName,
      version,
      description,
      sourcing_mode,
      external_git_url,
      selected_skills: rawSkills,
      selected_agents: rawAgents,
      selected_commands: rawCommands,
      hooks_json: hooksRaw,
      mcp_servers_json: mcpServersRaw,
      lsp_servers_json: lspServersRaw,
      monitors_json: monitorsRaw,
    };

    // --- Manifest-side validation ----------------------------------------

    if (!slug) return fail(400, { ...echo, error: 'Slug is required.' });
    if (!SLUG_RE.test(slug) || slug.length > 64) {
      return fail(400, {
        ...echo,
        error: 'Slug must be kebab-case (lowercase letters, digits, hyphens), 1–64 characters.',
      });
    }
    if (!version) return fail(400, { ...echo, error: 'Version is required.' });
    if (!SEMVER_RE.test(version)) {
      return fail(400, { ...echo, error: 'Version must be a semver string (e.g. 1.2.0).' });
    }
    if (!description) {
      return fail(400, { ...echo, error: 'Description is required.' });
    }
    if (!['internal', 'external', 'mirror'].includes(sourcing_mode)) {
      return fail(400, { ...echo, error: 'Invalid sourcing mode.' });
    }
    if ((sourcing_mode === 'external' || sourcing_mode === 'mirror') && !external_git_url) {
      return fail(400, {
        ...echo,
        error: `${sourcing_mode} sourcing requires an external git URL.`,
      });
    }
    if (external_git_url) {
      try {
        new URL(external_git_url);
      } catch {
        return fail(400, { ...echo, error: 'External git URL must be a valid URL.' });
      }
    }

    // --- Contents --------------------------------------------------------

    const contents = [
      ...parseSelected(rawSkills, 'skill'),
      ...parseSelected(rawAgents, 'agent'),
      ...parseSelected(rawCommands, 'command'),
    ];
    if (contents.length === 0) {
      return fail(400, {
        ...echo,
        error: 'Pick at least one skill, agent, or command for the plugin to bundle.',
      });
    }
    if (contents.length > MAX_CONTENTS) {
      return fail(400, {
        ...echo,
        error: `Too many items (${contents.length}). Plugins are capped at ${MAX_CONTENTS}.`,
      });
    }

    // --- Inline JSON blobs ----------------------------------------------

    const hooks = parseInlineJson(hooksRaw, 'hooks');
    if (!hooks.ok) return fail(400, { ...echo, error: hooks.error, section: hooks.section });
    const mcpServers = parseInlineJson(mcpServersRaw, 'mcpServers');
    if (!mcpServers.ok)
      return fail(400, { ...echo, error: mcpServers.error, section: mcpServers.section });
    const lspServers = parseInlineJson(lspServersRaw, 'lspServers');
    if (!lspServers.ok)
      return fail(400, { ...echo, error: lspServers.error, section: lspServers.section });
    const monitors = parseInlineJson(monitorsRaw, 'monitors');
    if (!monitors.ok)
      return fail(400, { ...echo, error: monitors.error, section: monitors.section });

    // --- Manifest assembly ----------------------------------------------

    const manifest: Record<string, unknown> = {
      name: slug,
      version,
      description,
    };
    if (displayName) manifest.displayName = displayName;
    if (hooks.value !== undefined) manifest.hooks = hooks.value;
    if (mcpServers.value !== undefined) manifest.mcpServers = mcpServers.value;
    if (lspServers.value !== undefined) manifest.lspServers = lspServers.value;
    if (monitors.value !== undefined) {
      manifest.experimental = { monitors: monitors.value };
    }

    const body: PublishPluginBody = {
      slug,
      manifest,
      contents,
      sourcing_mode,
      status: 'published',
      ...(external_git_url ? { external_git_url } : {}),
    };

    let result;
    try {
      result = await publishPlugin(auth, body);
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : String(e);
      return fail(500, { ...echo, error: `Publish failed: ${msg}` });
    }
    if (!result.ok) {
      return fail(result.status, { ...echo, error: result.error });
    }

    redirect(303, `/admin/plugins/${encodeURIComponent(result.plugin.slug)}`);
  },
};
