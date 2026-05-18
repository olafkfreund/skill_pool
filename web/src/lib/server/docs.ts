/**
 * Help & Docs loader.
 *
 * Markdown files live in the repo's top-level `docs/` folder. Vite's
 * `import.meta.glob` bundles them into the build at compile time, so:
 *   - the portal always ships in lockstep with the docs in the repo
 *   - changes to `docs/*.md` show up after the next `npm run build`
 *   - there is no runtime disk read, no second copy to drift
 *
 * Two categories are split for the portal UI:
 *   - top-level `docs/*.md`         → "Documentation"
 *   - `docs/examples/*.md`          → "Real-life examples"
 */

import { marked } from 'marked';

const TOP_LEVEL_GLOB = import.meta.glob('../../../../docs/*.md', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

const EXAMPLE_GLOB = import.meta.glob('../../../../docs/examples/*.md', {
  query: '?raw',
  import: 'default',
  eager: true,
}) as Record<string, string>;

export type DocCategory = 'reference' | 'example';

export interface DocSummary {
  slug: string;
  title: string;
  excerpt: string;
  category: DocCategory;
}

export interface DocPage extends DocSummary {
  html: string;
}

interface RawDoc {
  slug: string;
  raw: string;
  category: DocCategory;
}

function fileNameToSlug(path: string): string {
  // path looks like '../../../../docs/api.md' or '.../examples/foo.md'.
  const base = path.split('/').pop() ?? path;
  return base.replace(/\.md$/i, '');
}

function loadCategory(glob: Record<string, string>, category: DocCategory): RawDoc[] {
  return Object.entries(glob)
    .map(([path, raw]) => ({ slug: fileNameToSlug(path), raw, category }))
    .sort((a, b) => a.slug.localeCompare(b.slug));
}

function allDocs(): RawDoc[] {
  return [...loadCategory(TOP_LEVEL_GLOB, 'reference'), ...loadCategory(EXAMPLE_GLOB, 'example')];
}

/**
 * Title = first `# heading` in the body. Falls back to a humanised slug.
 * Excerpt = first non-heading paragraph (capped at 240 chars).
 */
function deriveTitleAndExcerpt(raw: string, slug: string): { title: string; excerpt: string } {
  const lines = raw.split('\n');
  let title = '';
  let excerpt = '';
  for (const line of lines) {
    if (!title) {
      const m = /^#\s+(.+)$/.exec(line);
      if (m) {
        title = m[1].trim();
        continue;
      }
    } else if (!excerpt) {
      const trimmed = line.trim();
      if (trimmed.length === 0) continue;
      if (trimmed.startsWith('#')) continue;
      if (trimmed.startsWith('```')) continue;
      if (trimmed.startsWith('-') || trimmed.startsWith('*')) continue;
      excerpt = trimmed;
      break;
    }
  }
  if (!title) {
    title = slug
      .split('-')
      .map((s) => (s.length > 0 ? s[0].toUpperCase() + s.slice(1) : s))
      .join(' ');
  }
  if (excerpt.length > 240) {
    excerpt = excerpt.slice(0, 237) + '…';
  }
  return { title, excerpt };
}

export function listDocs(): DocSummary[] {
  return allDocs().map((d) => {
    const { title, excerpt } = deriveTitleAndExcerpt(d.raw, d.slug);
    return { slug: d.slug, title, excerpt, category: d.category };
  });
}

export function loadDoc(slug: string): DocPage | null {
  const match = allDocs().find((d) => d.slug === slug);
  if (!match) return null;
  const { title, excerpt } = deriveTitleAndExcerpt(match.raw, match.slug);
  // `marked` is fine synchronous when no async extensions are wired.
  const html = marked.parse(match.raw, { async: false }) as string;
  return { slug: match.slug, title, excerpt, html, category: match.category };
}
