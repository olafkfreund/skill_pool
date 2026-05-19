import { sveltekit } from '@sveltejs/kit/vite';
import { svelteTesting } from '@testing-library/svelte/vite';
import { defineConfig } from 'vitest/config';

// Vitest config for the web portal's regression tests.
//
// happy-dom is the environment of choice — it's an order of magnitude
// faster than jsdom for our purposes (no real layout, no real CSSOM, no
// real network). The trade-off is fidelity: axe-core in happy-dom catches
// missing ARIA, semantic structure, and label issues, but skips checks
// that depend on computed style (e.g. real colour contrast in pixel form).
// We compensate by also running our own `checkThemeContrast` helper, which
// computes WCAG contrast from theme hex values directly.
//
// `include` is deliberately narrow — only files under `tests/**` are
// considered tests, so test files won't accidentally collide with
// SvelteKit's own conventions (which use `.test.ts` next to source).
export default defineConfig({
  plugins: [sveltekit(), svelteTesting()],
  test: {
    environment: 'happy-dom',
    include: ['tests/**/*.test.ts'],
    globals: false,
    css: false,
    setupFiles: ['./tests/setup.ts'],
  },
});
