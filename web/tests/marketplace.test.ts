/**
 * Tests for the public marketplace browser (issue #35).
 *
 * Two pages exercised:
 *   - /marketplace              (list)
 *   - /marketplace/[slug]       (detail)
 *
 * Contract assertions:
 *   - Pages render without authentication-required content.
 *   - Install command text matches the spec: `/plugin marketplace add <installBase>`.
 *   - No admin actions (Edit, Archive, New plugin, Import) are visible.
 *   - Structured data (JSON-LD) is present on the detail page.
 *   - Empty state renders correctly when no plugins are published.
 *   - Card visual hierarchy: name, description, content counts, tags, copy button.
 */

import { afterEach, describe, expect, it } from 'vitest';
import { cleanup, render, screen } from '@testing-library/svelte';

import MarketplaceListPage from '../src/routes/(public)/marketplace/+page.svelte';
import MarketplaceDetailPage from '../src/routes/(public)/marketplace/[slug]/+page.svelte';

import type { Plugin, PluginDetail, PluginVersionRow } from '../src/lib/server/api';

// --- Fixtures ---------------------------------------------------------------

const INSTALL_BASE = 'https://acme.skill-pool.example.com';

const THREE_PLUGINS: Plugin[] = [
  {
    slug: 'rust-axum-toolkit',
    version: '1.2.0',
    name: 'Rust + Axum Toolkit',
    description: 'Curated skills for Rust + Axum web services.',
    status: 'published',
    sourcing_mode: 'internal',
    tags: ['rust', 'axum', 'web'],
    created_at: '2026-04-01T00:00:00Z',
  },
  {
    slug: 'acme-formatter',
    version: '0.4.1',
    name: 'Acme Formatter',
    description: 'Code formatter bundle from acme-corp.',
    status: 'published',
    sourcing_mode: 'mirror',
    tags: ['formatter'],
    created_at: '2026-04-02T00:00:00Z',
  },
  {
    slug: 'external-linter',
    version: '2.0.0',
    name: 'External Linter',
    description: null,
    status: 'published',
    sourcing_mode: 'external',
    tags: [],
    created_at: '2026-04-03T00:00:00Z',
  },
];

const PLUGIN_DETAIL: PluginDetail = {
  slug: 'rust-axum-toolkit',
  version: '1.2.0',
  name: 'Rust + Axum Toolkit',
  description: 'Curated skills for Rust + Axum web services.',
  status: 'published',
  sourcing_mode: 'internal',
  manifest: {
    name: 'rust-axum-toolkit',
    version: '1.2.0',
    description: 'Curated skills for Rust + Axum web services.',
  },
  contents: [
    { kind: 'skill', slug: 'rust-error-handling', version: '1.0.0', position: 0 },
    { kind: 'skill', slug: 'rust-axum-handler', version: '0.3.0', position: 1 },
    { kind: 'agent', slug: 'sql-migration-reviewer', version: '1.1.0', position: 2 },
    { kind: 'command', slug: 'release-notes', version: '0.2.0', position: 3 },
  ],
  created_at: '2026-04-01T00:00:00Z',
  updated_at: '2026-05-15T00:00:00Z',
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

const JSON_LD = JSON.stringify({
  '@context': 'https://schema.org',
  '@type': 'SoftwareApplication',
  name: 'Rust + Axum Toolkit',
  description: 'Curated skills for Rust + Axum web services.',
  softwareVersion: '1.2.0',
  applicationCategory: 'DeveloperApplication',
  operatingSystem: 'Any',
  url: `${INSTALL_BASE}/marketplace/rust-axum-toolkit`,
});

/* eslint-disable @typescript-eslint/no-explicit-any */

afterEach(() => cleanup());

// --- /marketplace list page -----------------------------------------------

describe('marketplace list page', () => {
  it('renders plugin cards with name, description, and tags', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, installBase: INSTALL_BASE } as any,
      },
    });
    expect(screen.getByRole('heading', { name: /Rust \+ Axum Toolkit/i })).toBeInTheDocument();
    expect(screen.getByRole('heading', { name: /Acme Formatter/i })).toBeInTheDocument();
    expect(screen.getByText(/Curated skills for Rust \+ Axum/i)).toBeInTheDocument();
  });

  it('install command matches the spec: /plugin marketplace add <installBase>', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, installBase: INSTALL_BASE } as any,
      },
    });
    const expectedCommand = `/plugin marketplace add ${INSTALL_BASE}`;
    // The command appears in the header code element (aria-label).
    const codeEl = document.querySelector('[aria-label="Marketplace install command"]');
    expect(codeEl?.textContent?.trim()).toBe(expectedCommand);
  });

  it('renders a copy button for the marketplace install command', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, installBase: INSTALL_BASE } as any,
      },
    });
    expect(
      screen.getByRole('button', { name: /Copy marketplace install command/i }),
    ).toBeInTheDocument();
  });

  it('renders per-card copy install buttons with accessible labels', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, installBase: INSTALL_BASE } as any,
      },
    });
    expect(
      screen.getByRole('button', { name: /Copy install command for Rust \+ Axum Toolkit/i }),
    ).toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: /Copy install command for Acme Formatter/i }),
    ).toBeInTheDocument();
  });

  it('shows the empty state when no plugins are published', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: [], installBase: INSTALL_BASE } as any,
      },
    });
    expect(
      screen.getByText(/No plugins have been published to this marketplace yet/i),
    ).toBeInTheDocument();
  });

  it('does not show admin actions (Edit, Archive, New plugin, Import)', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, installBase: INSTALL_BASE } as any,
      },
    });
    expect(screen.queryByRole('link', { name: /New plugin/i })).toBeNull();
    expect(screen.queryByRole('link', { name: /Import plugin/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /Archive/i })).toBeNull();
    expect(screen.queryByRole('link', { name: /^Edit$/i })).toBeNull();
  });

  it('surfaces an API error banner without admin links', () => {
    render(MarketplaceListPage, {
      props: {
        data: {
          plugins: [],
          installBase: INSTALL_BASE,
          error: 'Could not load plugins: 503',
        } as any,
      },
    });
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByText(/Could not load plugins: 503/i)).toBeInTheDocument();
  });

  it('renders "View details" links pointing to /marketplace/[slug]', () => {
    render(MarketplaceListPage, {
      props: {
        data: { plugins: THREE_PLUGINS, installBase: INSTALL_BASE } as any,
      },
    });
    const viewLinks = screen.getAllByRole('link', { name: /View details/i });
    expect(viewLinks.length).toBe(THREE_PLUGINS.length);
    expect(viewLinks[0].getAttribute('href')).toBe('/marketplace/rust-axum-toolkit');
  });
});

// --- /marketplace/[slug] detail page ---------------------------------------

describe('marketplace detail page', () => {
  it('renders the plugin name and version in the header', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    expect(screen.getByRole('heading', { name: /Rust \+ Axum Toolkit/i })).toBeInTheDocument();
    // Version appears in both the header metadata span and the version history table.
    expect(screen.getAllByText(/v1\.2\.0/).length).toBeGreaterThanOrEqual(1);
  });

  it('install command text matches /plugin marketplace add <installBase>', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    const expectedCommand = `/plugin marketplace add ${INSTALL_BASE}`;
    const input = document.querySelector(
      'input[aria-label="Install command"]',
    ) as HTMLInputElement | null;
    expect(input?.value).toBe(expectedCommand);
  });

  it('has a copy button for the install command', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    expect(screen.getByRole('button', { name: /Copy install command/i })).toBeInTheDocument();
  });

  it('splits contents into skill, agent, and command sub-tables', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    expect(screen.getByText('rust-error-handling')).toBeInTheDocument();
    expect(screen.getByText('sql-migration-reviewer')).toBeInTheDocument();
    expect(screen.getByText('release-notes')).toBeInTheDocument();
    // Section headers include counts.
    expect(screen.getByText(/Skills · 2/i)).toBeInTheDocument();
    expect(screen.getByText(/Agents · 1/i)).toBeInTheDocument();
    expect(screen.getByText(/Commands · 1/i)).toBeInTheDocument();
  });

  it('renders the manifest as JSON in a pre block', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    expect(screen.getByText(/"name": "rust-axum-toolkit"/)).toBeInTheDocument();
  });

  it('renders the version history table', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    // v1.2.0 appears in the header metadata AND the version history table.
    expect(screen.getAllByText(/v1\.2\.0/).length).toBeGreaterThanOrEqual(1);
    // v1.1.0 only appears in the version history table.
    expect(screen.getByText(/v1\.1\.0/)).toBeInTheDocument();
  });

  it('does not show admin actions on the detail page', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    expect(screen.queryByRole('button', { name: /^Archive/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /Save schedule/i })).toBeNull();
    expect(screen.queryByRole('link', { name: /^Edit$/i })).toBeNull();
  });

  it('has a breadcrumb navigation back to /marketplace', () => {
    render(MarketplaceDetailPage, {
      props: {
        data: {
          plugin: PLUGIN_DETAIL,
          versions: VERSIONS,
          installBase: INSTALL_BASE,
          jsonLd: JSON_LD,
        } as any,
      },
    });
    const breadcrumbLink = screen.getByRole('link', { name: /^Marketplace$/i });
    expect(breadcrumbLink.getAttribute('href')).toBe('/marketplace');
  });
});
