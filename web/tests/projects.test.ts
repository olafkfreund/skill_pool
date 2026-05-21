/**
 * Regression tests for the admin Projects + Plans surfaces.
 *
 * Closes the "no Vitest unit tests against the Svelte components for the
 * Projects/Plans editors" gap called out in
 * docs/project-review-2026-05-20.md (§6, gap #4).
 *
 * Strategy mirrors tests/a11y.test.ts: render each page-component with a
 * minimal `data`/`form` props object that satisfies the loader contract,
 * then assert on observable, behavioural markers a curator would notice
 * break — toast text, role-gated controls, empty states, version-history
 * action buttons, error banners. svelte-check already covers the types;
 * these tests cover the part svelte-check cannot — that the component
 * renders the right thing for the data it's actually given.
 *
 * Three pages exercised:
 *   - /admin/projects                  (list)
 *   - /admin/projects/new              (create form)
 *   - /admin/projects/[slug]           (metadata + tags + plan + items editor)
 */

import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/svelte';

import ProjectsListPage from '../src/routes/(authed)/admin/projects/+page.svelte';
import NewProjectPage from '../src/routes/(authed)/admin/projects/new/+page.svelte';
import ProjectDetailPage from '../src/routes/(authed)/admin/projects/[slug]/+page.svelte';

import type {
  Project,
  ProjectDetail,
  ProjectPlan,
  ProjectPlanVersion,
} from '../src/lib/server/api';

// --- Fixtures ---------------------------------------------------------------

const TWO_PROJECTS: Project[] = [
  {
    slug: 'acme-billing',
    name: 'Acme Billing',
    description: null,
    git_remote: 'https://github.com/acme/billing',
    stack_tags: ['rust', 'axum'],
    item_count: 3,
    plan_auto_refresh_interval_secs: null,
    created_at: '2026-04-01T00:00:00Z',
    updated_at: '2026-05-15T00:00:00Z',
  },
  {
    slug: 'acme-frontend',
    name: 'Acme Frontend',
    description: 'Web portal',
    git_remote: null,
    stack_tags: [],
    item_count: 0,
    plan_auto_refresh_interval_secs: null,
    created_at: '2026-04-02T00:00:00Z',
    updated_at: '2026-05-16T00:00:00Z',
  },
];

const PROJECT_WITH_ITEMS: ProjectDetail = {
  slug: 'acme-billing',
  name: 'Acme Billing',
  description: 'Stripe-backed billing flow',
  git_remote: 'https://github.com/acme/billing',
  stack_tags: ['rust', 'axum', 'stripe'],
  item_count: 3,
  plan_auto_refresh_interval_secs: 3600,
  created_at: '2026-04-01T00:00:00Z',
  updated_at: '2026-05-15T00:00:00Z',
  items: [
    { skill_slug: 'rust-error-handling', kind: 'skill' },
    { skill_slug: 'sql-migration-reviewer', kind: 'agent' },
    { skill_slug: 'release-notes', kind: 'command' },
  ],
};

const ACTIVE_PLAN: ProjectPlan = {
  version: 4,
  body_md: '# Q2 Roadmap\n\n- Migrate to v2 webhooks\n- Add retry queue',
  source_type: 'url',
  source_url: 'https://confluence.acme.com/projects/billing/plan',
  imported_at: '2026-05-20T12:00:00Z',
  imported_by_email: 'curator@acme.com',
  status: 'active',
};

const STALE_PLAN: ProjectPlan = {
  ...ACTIVE_PLAN,
  fetch_error: 'HTTP 502 from upstream',
  fetch_error_at: '2026-05-21T08:00:00Z',
};

const PLAN_VERSIONS: ProjectPlanVersion[] = [
  {
    version: 4,
    status: 'active',
    source_type: 'url',
    source_url: 'https://confluence.acme.com/projects/billing/plan',
    imported_at: '2026-05-20T12:00:00Z',
    imported_by_email: 'curator@acme.com',
  },
  {
    version: 3,
    status: 'superseded',
    source_type: 'url',
    source_url: 'https://confluence.acme.com/projects/billing/plan',
    imported_at: '2026-05-10T12:00:00Z',
    imported_by_email: 'curator@acme.com',
  },
];

// Page fixtures fold layout-level fields the loader doesn't return (theme,
// pendingDrafts) under `as any` for the same reason a11y.test.ts does:
// constructing a strictly-typed PageData here would duplicate generated
// SvelteKit types and break on every `svelte-kit sync`. The pages only
// read what's listed in the inline data objects.
/* eslint-disable @typescript-eslint/no-explicit-any */

afterEach(() => cleanup());

// --- Projects list ----------------------------------------------------------

describe('admin/projects (list)', () => {
  it('renders the empty state when there are no projects', () => {
    render(ProjectsListPage, {
      props: { data: { projects: [] } as any, form: null },
    });
    expect(screen.getByText(/No projects yet/i)).toBeInTheDocument();
  });

  it('renders one row per project with name, slug, git remote, and item count', () => {
    render(ProjectsListPage, {
      props: { data: { projects: TWO_PROJECTS } as any, form: null },
    });
    expect(screen.getByText('Acme Billing')).toBeInTheDocument();
    expect(screen.getByText('Acme Frontend')).toBeInTheDocument();
    expect(screen.getByText('acme-billing')).toBeInTheDocument();
    expect(screen.getByText('https://github.com/acme/billing')).toBeInTheDocument();
    expect(screen.getByText(/3\s+items/)).toBeInTheDocument();
  });

  it('surfaces the deletion success toast when form.deleted is set', () => {
    render(ProjectsListPage, {
      props: {
        data: { projects: TWO_PROJECTS } as any,
        form: { deleted: true, slug: 'acme-billing' } as any,
      },
    });
    expect(screen.getByText(/Deleted project/i)).toBeInTheDocument();
  });

  it('surfaces the load-error banner when data.error is set', () => {
    render(ProjectsListPage, {
      props: {
        data: { projects: [], error: 'Backend offline' } as any,
        form: null,
      },
    });
    expect(screen.getByText(/Backend offline/i)).toBeInTheDocument();
  });
});

// --- New project ------------------------------------------------------------

describe('admin/projects/new', () => {
  it('renders the slug + name fields and the create button', () => {
    render(NewProjectPage, { props: { form: null } });
    expect(screen.getByLabelText(/Slug/i)).toBeInTheDocument();
    expect(screen.getByLabelText(/^\s*Name/i)).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /Create project/i })).toBeInTheDocument();
  });

  it('re-populates field values from a rejected submission', () => {
    render(NewProjectPage, {
      props: {
        form: {
          error: 'Slug already exists.',
          slug: 'taken',
          name: 'Taken',
          description: 'a description',
          git_remote: null,
        } as any,
      },
    });
    expect((screen.getByLabelText(/Slug/i) as HTMLInputElement).value).toBe('taken');
    expect((screen.getByLabelText(/^\s*Name/i) as HTMLInputElement).value).toBe('Taken');
    expect(screen.getByText(/Slug already exists/i)).toBeInTheDocument();
  });
});

// --- Project detail (metadata / tags / plan / items) -----------------------

describe('admin/projects/[slug] — empty plan state', () => {
  it('shows the "No plan imported yet" empty card when data.plan is null', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: null,
          planVersions: [],
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    expect(screen.getByText(/No plan imported yet/i)).toBeInTheDocument();
    // The curator import-command hint should be visible.
    expect(screen.getByText(/skill-pool plan import/i)).toBeInTheDocument();
  });
});

describe('admin/projects/[slug] — active plan card', () => {
  it('renders version chip, status, source link, and plan body', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: ACTIVE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    // "v4" appears both in the active card chip and the history-row cell.
    // We only care that the version is surfaced at all — fixed-count is too
    // tightly coupled to layout choices.
    expect(screen.getAllByText(/v4/).length).toBeGreaterThan(0);
    // status pill — several "active" tokens (one in the card top row, one
    // in the version-history row); just assert at least one.
    expect(screen.getAllByText(/active/i).length).toBeGreaterThan(0);
    expect(screen.getByText(/Q2 Roadmap/)).toBeInTheDocument();
    const sourceLink = screen
      .getAllByRole('link')
      .find((a) => a.getAttribute('href') === ACTIVE_PLAN.source_url);
    expect(sourceLink, 'plan source URL must render as a link').toBeTruthy();
  });

  it('renders the stale-fetch warning chip when plan.fetch_error is set', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: STALE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    expect(screen.getByText(/Last refresh failed: HTTP 502/i)).toBeInTheDocument();
  });

  it('shows an Activate button only on superseded versions in the history table', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: ACTIVE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    const activateButtons = screen.getAllByRole('button', { name: /^Activate$/i });
    // Two versions in the fixture: v4 (active → no button) and v3 (superseded → one button).
    expect(activateButtons).toHaveLength(1);
  });
});

describe('admin/projects/[slug] — role-gating', () => {
  it('shows the auto-refresh form to curators', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: ACTIVE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    expect(screen.getByLabelText(/Auto-refresh from source/i)).toBeInTheDocument();
  });

  it('shows the auto-refresh form to admins', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: ACTIVE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'admin',
        } as any,
        form: null,
      },
    });
    expect(screen.getByLabelText(/Auto-refresh from source/i)).toBeInTheDocument();
  });

  it('hides the auto-refresh form from publishers', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: ACTIVE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'publisher',
        } as any,
        form: null,
      },
    });
    expect(screen.queryByLabelText(/Auto-refresh from source/i)).toBeNull();
  });

  it('hides the auto-refresh form from viewers', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: ACTIVE_PLAN,
          planVersions: PLAN_VERSIONS,
          userRole: 'viewer',
        } as any,
        form: null,
      },
    });
    expect(screen.queryByLabelText(/Auto-refresh from source/i)).toBeNull();
  });
});

describe('admin/projects/[slug] — curated items', () => {
  it('splits items into skill / agent / command rows', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: null,
          planVersions: [],
          userRole: 'curator',
        } as any,
        form: null,
      },
    });
    expect(screen.getByText('rust-error-handling')).toBeInTheDocument();
    expect(screen.getByText('sql-migration-reviewer')).toBeInTheDocument();
    expect(screen.getByText('release-notes')).toBeInTheDocument();
  });
});

describe('admin/projects/[slug] — toasts and errors', () => {
  it('surfaces the meta-saved success toast', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: null,
          planVersions: [],
          userRole: 'curator',
        } as any,
        form: { action: 'meta', saved: true } as any,
      },
    });
    expect(screen.getByText(/Metadata saved/i)).toBeInTheDocument();
  });

  it('surfaces a server-side error banner', () => {
    render(ProjectDetailPage, {
      props: {
        data: {
          project: PROJECT_WITH_ITEMS,
          plan: null,
          planVersions: [],
          userRole: 'curator',
        } as any,
        form: {
          action: 'addItem',
          kind: 'skill',
          error: 'skill "foo" is already in this project.',
        } as any,
      },
    });
    expect(screen.getByText(/skill "foo" is already in this project/i)).toBeInTheDocument();
  });
});
