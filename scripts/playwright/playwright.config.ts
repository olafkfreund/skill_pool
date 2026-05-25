import { defineConfig, devices } from '@playwright/test';

/**
 * Playwright config for the onboarding-portal showcase recording.
 *
 * One project, headed Chromium, fixed viewport, video always captured.
 * Output webm lands under test-results/; the orchestrator copies it to
 * docs/demo/onboarding-portal.webm.
 */
export default defineConfig({
  testDir: '.',
  timeout: 120_000,
  expect: { timeout: 10_000 },
  fullyParallel: false,
  retries: 0,
  workers: 1,
  reporter: 'line',
  use: {
    baseURL: process.env.PORTAL_BASE_URL ?? 'http://127.0.0.1:3030',
    headless: true,
    viewport: { width: 1400, height: 760 },
    video: { mode: 'on', size: { width: 1400, height: 760 } },
    // Slow down each action so the recording reads as a deliberate walkthrough,
    // not a JS-fast click storm.
    actionTimeout: 10_000,
  },
  projects: [
    {
      name: 'onboarding-portal',
      // Use the system-installed Google Chrome (NixOS exposes `google-chrome`)
      // instead of downloading Playwright's pinned chromium binary. The
      // downloaded chromium expects Debian/Ubuntu paths (libnspr4.so etc.)
      // and refuses to launch on NixOS without a buildFHSUserEnv wrapper.
      use: {
        ...devices['Desktop Chrome'],
        // Use the system-installed Google Chrome explicitly. The path is
        // overridable via $SP_DEMO_CHROME for non-NixOS users (where
        // `which google-chrome` will resolve to a regular FHS location).
        launchOptions: {
          // Resolved at runtime: the orchestrator sets SP_DEMO_CHROME via
          // `which google-chrome` so this works on any host where
          // google-chrome is on PATH.
          executablePath: process.env.SP_DEMO_CHROME,
        },
      },
    },
  ],
});
