/**
 * Accessibility regression test for the skill-pool web portal.
 *
 * Closes one box on issue #9 ("Accessibility regression test: snapshot
 * test that all themed pages remain WCAG AA on the supported palettes").
 *
 * Strategy:
 *   1. Render each page-component via @testing-library/svelte, passing a
 *      minimal `data`/`form` props object that satisfies the component's
 *      contract (matches what the +page.server.ts loaders return).
 *   2. Wrap the rendered HTML in a themed CSS-variables block (`:root
 *      { --sp-* ... }`) so axe-core, which sniffs computed style via
 *      happy-dom's CSSOM, sees the right colours.
 *   3. Run `axe-core` against the rendered DOM and assert that no
 *      `serious` or `critical` violations are present. Lower-severity
 *      issues (`minor`, `moderate`) are advisory; failing on them here
 *      would make the test noisy and discourage the team from running it.
 *   4. Independently run `checkThemeContrast` (the same helper used by the
 *      server on save) against each palette so the contrast guarantees we
 *      ship with stay locked.
 *
 * Pages exercised:
 *   - /login              (public)
 *   - /                   (authed, catalog landing)
 *   - /admin/theme        (authed, the theme editor itself)
 *   - /drafts             (authed, draft inbox)
 *
 * What this test does NOT catch (see docs/a11y-testing.md):
 *   - Issues that depend on real layout (overlapping elements, viewport).
 *   - Keyboard-only navigation flows.
 *   - Focus management across navigations.
 *   - Real-browser colour rendering (subpixel, gamma, etc.).
 *   For end-to-end fidelity, run axe-core/playwright in CI instead.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render } from '@testing-library/svelte';
import axe from 'axe-core';

import LoginPage from '../src/routes/(public)/login/+page.svelte';
import CatalogPage from '../src/routes/(authed)/+page.svelte';
import ThemePage from '../src/routes/(authed)/admin/theme/+page.svelte';
import DraftsPage from '../src/routes/(authed)/drafts/+page.svelte';

import { DEFAULT_THEME, themeToCss, type Theme } from '../src/lib/theme';
import { checkThemeContrast } from '../src/lib/contrast';

// --- Test fixtures ----------------------------------------------------------

/**
 * Three palettes that should always pass WCAG AA:
 *   1. The shipped default (light, bluish).
 *   2. A dark variant (the canonical "I just want dark mode" choice).
 *   3. A high-contrast variant for accessibility-first tenants.
 *
 * Each palette is exercised against every page so regressions in either
 * the palette or the markup get caught.
 */
const PALETTES: Array<{ name: string; theme: Theme }> = [
  {
    name: 'default (light)',
    theme: { ...DEFAULT_THEME },
  },
  {
    name: 'dark',
    theme: {
      ...DEFAULT_THEME,
      primary: '#60a5fa',
      primaryFg: '#0b1220',
      accent: '#22d3ee',
      bg: '#0f172a',
      fg: '#f1f5f9',
      muted: '#1e293b',
      mutedFg: '#cbd5e1',
      border: '#334155',
    },
  },
  {
    name: 'high-contrast',
    theme: {
      ...DEFAULT_THEME,
      primary: '#000000',
      primaryFg: '#ffffff',
      accent: '#0000ee',
      bg: '#ffffff',
      fg: '#000000',
      muted: '#f5f5f5',
      mutedFg: '#1a1a1a',
      border: '#000000',
    },
  },
];

const AXE_OPTIONS: axe.RunOptions = {
  // Run the WCAG 2.1 AA ruleset — what the rest of the product targets.
  // axe-core also ships some "best-practices" checks; we leave those off
  // here to keep the regression net firmly on accessibility-spec
  // violations and out of stylistic territory.
  runOnly: {
    type: 'tag',
    values: ['wcag2a', 'wcag2aa', 'wcag21a', 'wcag21aa'],
  },
  resultTypes: ['violations'],
};

const BLOCKING_IMPACTS = new Set<axe.ImpactValue>(['critical', 'serious']);

/**
 * Inject a `<style>` block carrying the theme's CSS variables into the
 * happy-dom document. This mirrors what +layout.svelte does at runtime
 * via `themeToCss`, so component output that references `var(--sp-...)`
 * resolves to the palette under test.
 */
function applyTheme(theme: Theme): void {
  const styleId = 'a11y-test-theme';
  const existing = document.getElementById(styleId);
  if (existing) existing.remove();
  const style = document.createElement('style');
  style.id = styleId;
  style.textContent = `:root { ${themeToCss(theme)} } body { background: var(--sp-bg); color: var(--sp-fg); }`;
  document.head.appendChild(style);
}

/**
 * Filter axe violations to the impact levels we treat as blocking.
 * Lower-severity findings are returned so a future change can surface
 * them as warnings, but the assertion only fails on serious/critical.
 */
function blockingViolations(results: axe.AxeResults): axe.Result[] {
  return results.violations.filter((v) => v.impact && BLOCKING_IMPACTS.has(v.impact));
}

/**
 * Render-and-axe helper. Each call produces a fresh document, themed,
 * with the page mounted. Returns the axe results so the caller can
 * assert on them.
 */
async function axePage(componentFactory: () => void, theme: Theme): Promise<axe.AxeResults> {
  applyTheme(theme);
  componentFactory();
  // axe-core reads the live document. Default `<html>` element is fine —
  // the testing-library render mounts the component as a child of body.
  return axe.run(document, AXE_OPTIONS);
}

// --- Page fixture data ------------------------------------------------------

// The fixtures below mimic the shapes that each `+page.server.ts`
// returns at load time. SvelteKit's generated `PageData` type also folds
// in layout-level fields (e.g. `theme`, `pendingDrafts`) that are
// supplied by the parent layout's loader. Constructing a fully-typed
// `PageData` here would require us to import generated types from
// `.svelte-kit/types/` — fragile across `svelte-kit sync` runs. We cast
// each fixture to `any` so the page accepts it; the component renders,
// axe runs, the test passes. The compile-time guarantee is the page
// itself; what we're protecting against is markup regression.

const LOGIN_DATA = {
  tenant: { slug: 'acme', authed: false },
  sso: { oidc: { enabled: false }, saml: { enabled: false }, anyEnabled: false },
  oidcStart: null,
  samlMetadataUrl: null,
  theme: DEFAULT_THEME,
  pendingDrafts: 0,
} as const;

const CATALOG_DATA = {
  tenant: { slug: 'acme', authed: true },
  theme: DEFAULT_THEME,
  pendingDrafts: 0,
  skills: [
    {
      slug: 'react-query-state-sync',
      version: '2.4.1',
      description: 'Bidirectional sync for TanStack Query caches across tabs.',
      tags: ['react', 'state'],
      status: 'published',
      created_at: '2026-01-01T00:00:00Z',
      similarity: null,
    },
  ],
  query: '',
  semantic: false,
  kind: 'skill',
  error: '',
} as const;

const THEME_DATA = {
  tenant: { slug: 'acme', authed: true },
  theme: DEFAULT_THEME,
  pendingDrafts: 0,
  hasLogo: false,
  hasFavicon: false,
  fonts: ['system', 'Inter', 'IBM Plex Sans'],
} as const;

const DRAFTS_DATA = {
  tenant: { slug: 'acme', authed: true },
  theme: DEFAULT_THEME,
  pendingDrafts: 0,
  status: 'pending',
  error: '',
  drafts: [
    {
      id: 'd1',
      slug: 'sample-draft',
      description: 'A draft awaiting review.',
      when_to_use: 'For demonstrating the test fixture.',
      tags: ['demo'],
      origin: 'cli',
      notes: null,
      status: 'pending',
      published_version: null,
      created_at: '2026-01-01T00:00:00Z',
      reviewed_at: null,
    },
  ],
} as const;

// --- Tests ------------------------------------------------------------------

describe('contrast guarantees', () => {
  for (const { name, theme } of PALETTES) {
    it(`palette "${name}" passes WCAG AA contrast (fg/bg, primary, muted)`, () => {
      const failures = checkThemeContrast(theme);
      expect(
        failures,
        `palette "${name}" has contrast failures: ${JSON.stringify(failures, null, 2)}`,
      ).toEqual([]);
    });
  }
});

// Renders use `as any` for the `data` prop. SvelteKit's generated
// `PageData` types fold together the route loader's return + every
// ancestor layout's loader return; staying type-strict here would
// duplicate that machinery. Test fixtures above cover the fields the
// component actually reads.
/* eslint-disable @typescript-eslint/no-explicit-any */

describe('a11y: login page', () => {
  afterEach(() => cleanup());
  for (const { name, theme } of PALETTES) {
    it(`no serious/critical axe violations on "${name}" palette`, async () => {
      const results = await axePage(
        () => render(LoginPage, { props: { data: LOGIN_DATA as any, form: null } }),
        theme,
      );
      const blocking = blockingViolations(results);
      expect(blocking, `login page (${name}) violations:\n${formatViolations(blocking)}`).toEqual(
        [],
      );
    });
  }
});

describe('a11y: catalog page', () => {
  afterEach(() => cleanup());
  for (const { name, theme } of PALETTES) {
    it(`no serious/critical axe violations on "${name}" palette`, async () => {
      const results = await axePage(
        () => render(CatalogPage, { props: { data: CATALOG_DATA as any } }),
        theme,
      );
      const blocking = blockingViolations(results);
      expect(blocking, `catalog page (${name}) violations:\n${formatViolations(blocking)}`).toEqual(
        [],
      );
    });
  }
});

describe('a11y: theme editor', () => {
  afterEach(() => cleanup());
  for (const { name, theme } of PALETTES) {
    it(`no serious/critical axe violations on "${name}" palette`, async () => {
      const results = await axePage(
        () => render(ThemePage, { props: { data: THEME_DATA as any, form: null } }),
        theme,
      );
      const blocking = blockingViolations(results);
      expect(blocking, `theme page (${name}) violations:\n${formatViolations(blocking)}`).toEqual(
        [],
      );
    });
  }
});

describe('a11y: drafts inbox', () => {
  afterEach(() => cleanup());
  for (const { name, theme } of PALETTES) {
    it(`no serious/critical axe violations on "${name}" palette`, async () => {
      const results = await axePage(
        () => render(DraftsPage, { props: { data: DRAFTS_DATA as any, form: null } }),
        theme,
      );
      const blocking = blockingViolations(results);
      expect(blocking, `drafts page (${name}) violations:\n${formatViolations(blocking)}`).toEqual(
        [],
      );
    });
  }
});

// --- Helpers ----------------------------------------------------------------

function formatViolations(violations: axe.Result[]): string {
  if (violations.length === 0) return '(none)';
  return violations
    .map((v) => {
      const targets = v.nodes
        .slice(0, 3)
        .map((n) => n.target.join(' '))
        .join('; ');
      return `  - [${v.impact}] ${v.id}: ${v.help} (targets: ${targets})`;
    })
    .join('\n');
}
