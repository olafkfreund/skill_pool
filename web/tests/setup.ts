// Shared test setup. Kept tiny — Svelte + Vitest already wires up the
// document and the @testing-library cleanup hook (via the
// `svelteTesting()` Vite plugin), so this file exists mainly as a hook
// for future shared mocks. Importing it from vitest.config.ts ensures the
// hook order is deterministic when more setup is added later.
import '@testing-library/jest-dom/vitest';
import { beforeEach } from 'vitest';

// happy-dom builds the document on demand and (sensibly) leaves
// document-level chrome to the test. The real portal serves an
// `<html lang="en">` + `<title>` via app.html, so we set those once
// here. axe-core's `html-has-lang` and `document-title` checks are
// serious-impact, but they're about the surrounding shell — failing on
// them here would be testing happy-dom's defaults, not our pages.
beforeEach(() => {
  document.documentElement.setAttribute('lang', 'en');
  if (!document.title) document.title = 'skill-pool';
});
