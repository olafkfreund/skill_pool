# Accessibility regression tests

The web portal ships with a Vitest-based accessibility regression suite
that runs `axe-core` against the four most-trafficked themed pages on
three different palettes (default, dark, high-contrast). It closes the
"Accessibility regression test" bullet on tracking issue
[#9](https://github.com/anthropics/skill-pool/issues/9).

The suite lives at `web/tests/a11y.test.ts`. Vitest config lives at
`web/vitest.config.ts`.

## What this test catches

For every combination of (page × palette) it asserts:

1. **No `serious` or `critical` axe-core violations.** axe runs the full
   WCAG 2.0/2.1 A + AA rule set (`wcag2a`, `wcag2aa`, `wcag21a`,
   `wcag21aa` tags). Typical catches:
   - `<input>` without an associated `<label>` or `aria-label`.
   - `<button>` with no discernible text content.
   - Color-only feedback (e.g. red text with no icon or sr-only label).
   - `<img>` without `alt`.
   - `aria-*` attributes referring to non-existent ids.
   - Landmark / heading-order regressions.
2. **All four WCAG AA contrast pairs pass** (`fg/bg`, `primaryFg/primary`,
   `mutedFg/muted`, `mutedFg/bg`). The same helper the server uses on
   save (`web/src/lib/contrast.ts`) is run against each test palette
   independently of axe — this gives a guarantee that's not subject to
   happy-dom's CSS computation quirks.

Pages covered:

- `/login` (public)
- `/` (catalog landing)
- `/admin/theme` (theme editor)
- `/drafts` (drafts inbox)

## What this test does NOT catch

`happy-dom` is a JavaScript-only DOM implementation; it is fast (the
whole suite runs in ~10s on a laptop) but it does not implement
everything a real browser does. In particular:

- **Real-browser colour contrast.** axe's `color-contrast` rule depends
  on computed style. happy-dom's CSSOM is approximate, so axe may skip
  or mis-evaluate this rule. We compensate with the dedicated
  `checkThemeContrast` block, but pixel-accurate contrast (with anti-
  aliasing, sub-pixel rendering) is only verifiable in a real browser.
- **Keyboard navigation flows.** Tab order, focus traps, and skip-links
  need an actual browser to evaluate. `axe-core/playwright` (Option 1
  in the [task brief](#why-happy-dom-and-not-playwright)) is the right
  tool for that.
- **Dynamic interactions.** Forms going through their full submit
  lifecycle, async state, error-banner appearance after a failed POST,
  etc. The test renders the page in its load-time state; if a
  branch only appears after a `form?.error` is set, you need a second
  fixture invocation or a Playwright test.
- **Animations & motion-reduced preferences.** Out of scope here.
- **Issues on routes we don't render.** Help, members, SSO admin, etc.
  are not in the regression net.

For full-fidelity accessibility coverage, the operator should run
`@axe-core/playwright` in their own CI pipeline against a deployed
preview — that catches everything happy-dom can't, at the cost of a
heavier setup. Treat the Vitest suite as the inner-loop regression net
and the Playwright suite (if you have one) as the outer-loop gate.

## Running locally

```sh
cd web
npm install   # only once
npm test               # run everything
npm run test:a11y      # run just the a11y suite (the only suite today)
npm run validate       # svelte-check + tests, the "is this PR shippable" gate
```

CI: there are no GitHub Actions in this repo yet. The
`npm run validate` script is the recommended local gate, and operators
running their own CI should invoke it from whatever runner they use.
Once GH Actions land, the validate target should be the test step.

## Why happy-dom and not Playwright

The task brief picked the lighter option deliberately. axe-core under
happy-dom catches the structural / ARIA / semantic bugs that make up the
bulk of regression-prone a11y issues, while staying in the unit-test
loop. A full-browser `@axe-core/playwright` setup would catch a strict
superset, but at the cost of:

- ~50 MB of Playwright browsers in `node_modules`.
- A multi-second cold start per spec (browser launch).
- A SvelteKit dev-server lifecycle to manage in the test runner.
- An extra hop in the test → debug cycle when something breaks.

If the team adopts Playwright for other reasons (smoke tests, visual
regression), it's worth promoting the a11y suite onto it. Until then,
happy-dom + axe-core is the right calibration.

## Adding a new page to the suite

1. Import the page component at the top of `tests/a11y.test.ts`:
   ```ts
   import NewPage from '../src/routes/(authed)/some/path/+page.svelte';
   ```
2. Add a fixture object alongside `CATALOG_DATA` / `THEME_DATA`. It
   needs to satisfy the page's `data` prop shape (look at the matching
   `+page.server.ts` to know what the loader returns). The fixture is
   cast to `any` at the render call-site so you don't need a
   pixel-perfect `PageData` type.
3. Add a new `describe(...)` block following the existing pattern. For
   each of the three palettes, render the page, run `axePage`, assert
   `blockingViolations` is empty.
4. If the page imports `$app/state` or `$app/navigation`, you may need
   to provide a `vi.mock(...)` for those modules. The four current
   pages don't — they read everything from the `data` prop, which is
   the easiest path. Pages that rely on `$page.url` for client-side
   routing are harder to mount and are currently out of scope.
5. Run `npm run test:a11y` and fix any violations the new page surfaces.

## Honest limitations / known gaps

- The dark and high-contrast palettes used in the test are not the
  exact palettes any specific customer ships. They cover the contrast
  *envelope* (very dark, very high-contrast); per-tenant saved palettes
  are validated on the server's `PUT /v1/theme` path via
  `checkThemeContrast`, so the contrast guarantee transitively holds
  for everyone who saves through the proper API.
- The `serious`/`critical` threshold is a calibration choice. Bumping
  it to include `moderate` and `minor` would surface a long tail of
  advisory issues (e.g. landmark naming, redundant-link descriptions);
  raise the bar once the existing list is at zero and you want to
  catch the next class of bug.
