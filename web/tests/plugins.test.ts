/**
 * Regression tests for the admin Plugins surfaces.
 *
 * Mirrors `web/tests/projects.test.ts`: render each page-component with
 * a minimal `data` / `form` props object that satisfies the loader
 * contract, then assert on observable, behavioural markers a curator
 * would notice break — empty states, role-gated controls, composer
 * validation messaging, mirror-only auto-refresh toggle, import-stub
 * banner.
 *
 * Four pages exercised:
 *   - /admin/plugins              (list)
 *   - /admin/plugins/new          (composer)
 *   - /admin/plugins/[slug]       (detail editor)
 *   - /admin/plugins/import       (mirror-import stub)
 */

import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/svelte';

import PluginsListPage from '../src/routes/(authed)/admin/plugins/+page.svelte';
import NewPluginPage from '../src/routes/(authed)/admin/plugins/new/+page.svelte';
import PluginDetailPage from '../src/routes/(authed)/admin/plugins/[slug]/+page.svelte';
import ImportPluginPage from '../src/routes/(authed)/admin/plugins/import/+page.svelte';

import type { Plugin, PluginDetail, PluginVersionRow } from '../src/lib/server/api';
import type { Skill } from '../src/lib/types';

// --- Fixtures ---------------------------------------------------------------

const THREE_PLUGINS: Plugin[] = [
  {
    slug: 'rust-axum-toolkit',
    version: '1.2.0',
    name: 'Rust + Axum Toolkit',
    description: 'Curated skills for Rust + Axum',
    status: 'published',
    sourcing_mode: 'internal',
    tags: ['rust', 'axum'],
    created_at: '2026-04-01T00:00:00Z',
  },
  {
    slug: 'acme-formatter',
    version: '0.4.1',
    name: 'Acme Formatter',
    description: 'Mirrored from acme-corp/formatter',
    status: 'published',
    sourcing_mode: 'mirror',
    tags: ['formatter'],
    created_at: '2026-04-02T00:00:00Z',
  },
  {
    slug: 'external-linter',
    version: '2.0.0',
    name: 'External Linter',
    description: 'Lives upstream, listed only',
    status: 'published',
    sourcing_mode: 'external',
    tags: [],
    created_at: '2026-04-03T00:00:00Z',
  },
];

const CATALOG_SKILLS: Skill[] = [
  {
    slug: 'rust-error-handling',
    version: '1.0.0',
    description: 'Idiomatic error handling in Rust',
    tags: ['rust'],
    status: 'published',
    created_at: '2026-01-01T00:00:00Z',
    similarity: null,
  },
  {
    slug: 'rust-axum-handler',
    version: '0.3.0',
    description: 'Axum request handlers',
    tags: ['rust', 'axum'],
    status: 'published',
    created_at: '2026-01-02T00:00:00Z',
    similarity: null,
  },
];

const CATALOG_AGENTS: Skill[] = [
  {
    slug: 'sql-migration-reviewer',
    version: '1.1.0',
    description: 'Reviews SQL migrations',
    tags: ['sql'],
    status: 'published',
    created_at: '2026-01-03T00:00:00Z',
    similarity: null,
  },
];

const CATALOG_COMMANDS: Skill[] = [
  {
    slug: 'release-notes',
    version: '0.2.0',
    description: 'Generate release notes',
    tags: [],
    status: 'published',
    created_at: '2026-01-04T00:00:00Z',
    similarity: null,
  },
];

const MIRROR_PLUGIN_DETAIL: PluginDetail = {
  slug: 'acme-formatter',
  version: '0.4.1',
  name: 'Acme Formatter',
  description: 'Mirrored from acme-corp/formatter',
  status: 'published',
  sourcing_mode: 'mirror',
  external_git_url: 'https://github.com/acme-corp/formatter',
  upstream_url: 'https://github.com/acme-corp/formatter',
  manifest: {
    name: 'acme-formatter',
    version: '0.4.1',
    description: 'Mirrored from acme-corp/formatter',
  },
  contents: [
    { kind: 'skill', slug: 'rust-error-handling', version: '1.0.0', position: 0 },
    { kind: 'agent', slug: 'sql-migration-reviewer', version: '1.1.0', position: 1 },
    { kind: 'command', slug: 'release-notes', version: '0.2.0', position: 2 },
  ],
  created_at: '2026-04-02T00:00:00Z',
  updated_at: '2026-05-15T00:00:00Z',
};

const INTERNAL_PLUGIN_DETAIL: PluginDetail = {
  ...MIRROR_PLUGIN_DETAIL,
  slug: 'rust-axum-toolkit',
  name: 'Rust + Axum Toolkit',
  sourcing_mode: 'internal',
  external_git_url: undefined,
  upstream_url: undefined,
  manifest: {
    name: 'rust-axum-toolkit',
    version: '1.2.0',
    description: 'Curated skills for Rust + Axum',
  },
};

const VERSIONS: PluginVersionRow[] = [
  {
    version: '1.2.0',
    status: 'published',
    created_at: '2026-05-15T00:00:00Z',
    published_by: 'curator@acme.com',
  },
  {
    version: '1.1.0',
    status: 'archived',
    created_at: '2026-05-01T00:00:00Z',
    published_by: 'curator@acme.com',
  },
];

const MARKETPLACE_URL = 'https://acme.skill-pool.example.com/.claude-plugin/marketplace.json';

// Page fixtures use `as any` for the same reason a11y.test.ts does:
// constructing a strictly-typed PageData here would duplicate generated
// SvelteKit types and break on `svelte-kit sync`. The pages only read
// what's listed in the inline data objects.
/* eslint-disable @typescript-eslint/no-explicit-any */

afterEach(() => cleanup());

// --- /admin/plugins list ---------------------------------------------------

describe('admin/plugins (list)', () => {
  it('renders the empty state when there are no plugins', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: [], sourcingMode: null, userRole: 'curator' } as any,
        form: null,
      },
    });
    expect(screen.getByText(/No plugins yet/i)).toBeInTheDocument();
  });

  it('renders one row per plugin with slug, version, and sourcing chip', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, sourcingMode: null, userRole: 'curator' } as any,
        form: null,
      },
    });
    expect(screen.getByText('rust-axum-toolkit')).toBeInTheDocument();
    expect(screen.getByText('acme-formatter')).toBeInTheDocument();
    expect(screen.getByText('external-linter')).toBeInTheDocument();
    expect(screen.getByText('internal')).toBeInTheDocument();
    expect(screen.getByText('mirror')).toBeInTheDocument();
    expect(screen.getByText('external')).toBeInTheDocument();
  });

  it('marks the active filter chip with aria-current when sourcingMode is set', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, sourcingMode: 'mirror', userRole: 'curator' } as any,
        form: null,
      },
    });
    // The Mirror chip is the current page when filtering by sourcing_mode=mirror.
    const mirrorChip = screen.getByRole('link', { name: /^Mirror$/i });
    expect(mirrorChip.getAttribute('aria-current')).toBe('page');
  });

  it('hides "+ New plugin" and "+ Import plugin" buttons from viewers', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, sourcingMode: null, userRole: 'viewer' } as any,
        form: null,
      },
    });
    expect(screen.queryByRole('link', { name: /New plugin/i })).toBeNull();
    expect(screen.queryByRole('link', { name: /Import plugin/i })).toBeNull();
  });

  it('shows "+ New plugin" and "+ Import plugin" buttons to curators', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, sourcingMode: null, userRole: 'curator' } as any,
        form: null,
      },
    });
    expect(screen.getByRole('link', { name: /New plugin/i })).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /Import plugin/i })).toBeInTheDocument();
  });

  it('shows action buttons to admins too', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, sourcingMode: null, userRole: 'admin' } as any,
        form: null,
      },
    });
    expect(screen.getByRole('link', { name: /New plugin/i })).toBeInTheDocument();
  });

  it('surfaces the archive success toast when form.archived is set', () => {
    render(PluginsListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, sourcingMode: null, userRole: 'curator' } as any,
        form: { archived: true, slug: 'rust-axum-toolkit', version: '1.2.0' } as any,
      },
    });
    expect(screen.getByText(/Archived plugin/i)).toBeInTheDocument();
  });
});

// --- /admin/plugins/new (composer) -----------------------------------------

describe('admin/plugins/new (composer)', () => {
  const FRESH_DATA = {
    skills: CATALOG_SKILLS,
    agents: CATALOG_AGENTS,
    commands: CATALOG_COMMANDS,
    userRole: 'curator',
  };

  it('renders the manifest fields and the publish button', () => {
    render(NewPluginPage, { props: { data: FRESH_DATA as any, form: null } });
    // `name=` is the most stable selector — labels in this composer
    // appear several times (the field's own label, a help block, the
    // results-column header).
    expect(document.querySelector('input[name="slug"]')).toBeInTheDocument();
    expect(document.querySelector('input[name="version"]')).toBeInTheDocument();
    expect(document.querySelector('textarea[name="description"]')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Publish plugin/i })).toBeInTheDocument();
  });

  it('renders the three-column type-ahead picker with each kind labelled', () => {
    render(NewPluginPage, { props: { data: FRESH_DATA as any, form: null } });
    expect(screen.getByLabelText(/Search skills/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/Search agents/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/Search commands/i)).toBeInTheDocument();
    // Initial render lists every catalogue entry as an add-button.
    expect(screen.getByRole('button', { name: /rust-error-handling/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /sql-migration-reviewer/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /release-notes/i })).toBeInTheDocument();
  });

  it('disables the publish button when nothing is selected', () => {
    render(NewPluginPage, { props: { data: FRESH_DATA as any, form: null } });
    const btn = screen.getByRole('button', { name: /Publish plugin/i });
    expect((btn as HTMLButtonElement).disabled).toBe(true);
  });

  it('re-populates form fields and surfaces a validation error from a rejected submission', () => {
    render(NewPluginPage, {
      props: {
        data: FRESH_DATA as any,
        form: {
          error: 'Slug must be kebab-case (lowercase letters, digits, hyphens), 1–64 characters.',
          slug: 'BadSlug',
          version: '1.0.0',
          description: 'a description',
          sourcing_mode: 'internal',
          external_git_url: '',
          selected_skills: 'rust-error-handling@1.0.0',
          selected_agents: '',
          selected_commands: '',
          hooks_json: '',
          mcp_servers_json: '',
          lsp_servers_json: '',
          monitors_json: '',
        } as any,
      },
    });
    const slugInput = document.querySelector('input[name="slug"]') as HTMLInputElement | null;
    expect(slugInput?.value).toBe('BadSlug');
    expect(screen.getByText(/must be kebab-case/i)).toBeInTheDocument();
  });

  it('highlights the offending JSON section when the action returns a section field', () => {
    render(NewPluginPage, {
      props: {
        data: FRESH_DATA as any,
        form: {
          error: 'hooks JSON: Unexpected token',
          section: 'hooks',
          slug: 'good-slug',
          version: '1.0.0',
          description: 'desc',
          sourcing_mode: 'internal',
          external_git_url: '',
          selected_skills: 'rust-error-handling@1.0.0',
          selected_agents: '',
          selected_commands: '',
          hooks_json: '{ not valid json',
          mcp_servers_json: '',
          lsp_servers_json: '',
          monitors_json: '',
        } as any,
      },
    });
    expect(screen.getByText(/hooks JSON: Unexpected token/i)).toBeInTheDocument();
    // The hooks textarea picks up the red-border class — assert it has *some* border that's red.
    const hooksTextarea = screen.getByLabelText(/^hooks$/i) as HTMLTextAreaElement;
    expect(hooksTextarea.className).toMatch(/red/);
  });
});

// --- /admin/plugins/[slug] detail ------------------------------------------

describe('admin/plugins/[slug] — manifest + marketplace URL', () => {
  it('renders the manifest as JSON and exposes the marketplace URL with a copy button', () => {
    render(PluginDetailPage, {
      props: {
        data: {
          plugin: INTERNAL_PLUGIN_DETAIL,
          versions: VERSIONS,
          marketplaceUrl: MARKETPLACE_URL,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    // Manifest preview renders the slug.
    expect(screen.getByText(/"name": "rust-axum-toolkit"/)).toBeInTheDocument();
    // Marketplace URL appears in the read-only input; both the input and
    // the button carry "Marketplace URL" so we query by attribute.
    const urlInput = document.querySelector(
      'input[aria-label="Marketplace URL"]',
    ) as HTMLInputElement | null;
    expect(urlInput?.value).toBe(MARKETPLACE_URL);
    expect(screen.getByRole('button', { name: /Copy marketplace URL/i })).toBeInTheDocument();
  });

  it('splits contents into skill / agent / command sub-tables', () => {
    render(PluginDetailPage, {
      props: {
        data: {
          plugin: INTERNAL_PLUGIN_DETAIL,
          versions: VERSIONS,
          marketplaceUrl: MARKETPLACE_URL,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    expect(screen.getByText('rust-error-handling')).toBeInTheDocument();
    expect(screen.getByText('sql-migration-reviewer')).toBeInTheDocument();
    expect(screen.getByText('release-notes')).toBeInTheDocument();
  });

  it('renders the version history table', () => {
    render(PluginDetailPage, {
      props: {
        data: {
          plugin: INTERNAL_PLUGIN_DETAIL,
          versions: VERSIONS,
          marketplaceUrl: MARKETPLACE_URL,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    expect(screen.getByText(/v1\.2\.0/)).toBeInTheDocument();
    expect(screen.getByText(/v1\.1\.0/)).toBeInTheDocument();
    // The active version gives the curator an Archive button; the
    // already-archived row collapses to a status pill.
    expect(screen.getByRole('button', { name: /^Archive$/i })).toBeInTheDocument();
  });
});

describe('admin/plugins/[slug] — role + sourcing-mode gating of mirror auto-refresh', () => {
  for (const role of ['curator', 'admin'] as const) {
    it(`shows the auto-refresh toggle to ${role} when sourcing_mode === 'mirror'`, () => {
      render(PluginDetailPage, {
        props: {
          data: {
            plugin: MIRROR_PLUGIN_DETAIL,
            versions: VERSIONS,
            marketplaceUrl: MARKETPLACE_URL,
            userRole: role,
          } as any,
          form: null,
        },
      });
      expect(screen.getByLabelText(/Auto-refresh from upstream/i)).toBeInTheDocument();
    });
  }

  for (const role of ['publisher', 'viewer'] as const) {
    it(`hides the auto-refresh toggle from ${role} even on a mirror plugin`, () => {
      render(PluginDetailPage, {
        props: {
          data: {
            plugin: MIRROR_PLUGIN_DETAIL,
            versions: VERSIONS,
            marketplaceUrl: MARKETPLACE_URL,
            userRole: role,
          } as any,
          form: null,
        },
      });
      expect(screen.queryByLabelText(/Auto-refresh from upstream/i)).toBeNull();
    });
  }

  for (const role of ['curator', 'admin', 'publisher', 'viewer'] as const) {
    it(`hides the auto-refresh toggle from ${role} when sourcing_mode is not 'mirror'`, () => {
      render(PluginDetailPage, {
        props: {
          data: {
            plugin: INTERNAL_PLUGIN_DETAIL,
            versions: VERSIONS,
            marketplaceUrl: MARKETPLACE_URL,
            userRole: role,
          } as any,
          form: null,
        },
      });
      expect(screen.queryByLabelText(/Auto-refresh from upstream/i)).toBeNull();
    });
  }
});

// --- /admin/plugins/import (stub) ------------------------------------------

describe('admin/plugins/import (stub)', () => {
  it('renders the "not yet available" banner with a link to tracking issue #32', () => {
    render(ImportPluginPage, { props: { form: null } });
    expect(screen.getByText(/not yet available/i)).toBeInTheDocument();
    // The tracking-issue link points at issue #32.
    const link = screen.getByRole('link', { name: /#32/i });
    expect(link.getAttribute('href')).toMatch(/issues\/32$/);
  });

  it('renders the git URL input + refresh interval input + enqueue button', () => {
    render(ImportPluginPage, { props: { form: null } });
    expect(screen.getByLabelText(/Git URL/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/Refresh interval/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Enqueue import/i })).toBeInTheDocument();
  });

  it('marks the inputs aria-disabled when the action signals notYetAvailable', () => {
    render(ImportPluginPage, {
      props: {
        form: {
          url: 'https://github.com/acme/formatter.git',
          refresh_interval_secs: '86400',
          error: 'Plugin import is not yet available',
          notYetAvailable: true,
          tracking_issue: 32,
        } as any,
      },
    });
    const urlInput = screen.getByLabelText(/Git URL/i);
    expect(urlInput.getAttribute('aria-disabled')).toBe('true');
  });

  it('surfaces a success toast with the job id once the worker is wired (#32)', () => {
    render(ImportPluginPage, {
      props: {
        form: {
          imported: true,
          job_id: 'job-abc-123',
          url: 'https://github.com/acme/formatter.git',
        } as any,
      },
    });
    expect(screen.getByText(/Import job enqueued/i)).toBeInTheDocument();
    expect(screen.getByText(/job-abc-123/)).toBeInTheDocument();
  });
});
