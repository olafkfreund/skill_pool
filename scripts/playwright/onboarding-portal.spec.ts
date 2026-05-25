import { test, expect } from '@playwright/test';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';

/**
 * Onboarding showcase — portal half.
 *
 * Walks the curator-side view of the same `acme-billing-service` project
 * a developer just `bootstrap`-ed in the CLI half:
 *
 *   1. /admin/projects     — list of curated projects
 *   2. /admin/projects/acme-billing-service — detail (items + active plan)
 *   3. scroll to show items + plan
 *   4. back to /admin/projects
 *
 * Output: test-results/onboarding-portal-onboarding-portal/video.webm
 * Renamed by the orchestrator to docs/demo/onboarding-portal.webm.
 *
 * Auth: cookies `sp_token` + `sp_tenant=acme`, sourced from
 * ~/.config/skill-pool/config.toml (or env SP_DEMO_TOKEN).
 */

function loadToken(): string {
  if (process.env.SP_DEMO_TOKEN) return process.env.SP_DEMO_TOKEN;
  const cfg = path.join(os.homedir(), '.config', 'skill-pool', 'config.toml');
  if (!fs.existsSync(cfg)) {
    throw new Error(
      `no token: set SP_DEMO_TOKEN or run scripts/seed-demo.sh to write ${cfg}`,
    );
  }
  const raw = fs.readFileSync(cfg, 'utf-8');
  const m = raw.match(/^\s*token\s*=\s*"([^"]+)"/m);
  if (!m) throw new Error(`no [registry].token in ${cfg}`);
  return m[1];
}

const TENANT = 'acme';
const TOKEN = loadToken();

test('onboarding-portal', async ({ page, context }) => {
  // 1. Inject admin session cookies BEFORE first navigation.
  const url = new URL(page.url() === 'about:blank' ? 'http://127.0.0.1:3030/' : page.url());
  const baseUrl = process.env.PORTAL_BASE_URL ?? 'http://127.0.0.1:3030';
  const domain = new URL(baseUrl).hostname;

  await context.addCookies([
    {
      name: 'sp_token',
      value: TOKEN,
      domain,
      path: '/',
      httpOnly: false,
      sameSite: 'Lax',
    },
    {
      name: 'sp_tenant',
      value: TENANT,
      domain,
      path: '/',
      httpOnly: false,
      sameSite: 'Lax',
    },
  ]);

  // 2. Land on /admin/projects — the curator's home for project bundles.
  await page.goto('/admin/projects');
  await expect(page).toHaveURL(/\/admin\/projects$/);
  // The page renders projects as rows in a table. Wait for the slug
  // cell to be visible — that's our "data has loaded" signal.
  const billingRow = page.locator('tr').filter({ hasText: 'acme-billing-service' });
  await expect(billingRow).toBeVisible({ timeout: 15_000 });
  await page.waitForTimeout(2_500); // hold the list view

  // 3. Click "Edit" on the billing-service row to drill in.
  await billingRow.getByRole('link', { name: /^Edit$/i }).click();
  await expect(page).toHaveURL(/\/admin\/projects\/acme-billing-service/);

  // 4. Wait for the curated items table + plan section to render.
  await expect(page.getByText(/Plan/i).first()).toBeVisible({ timeout: 10_000 });
  await page.waitForTimeout(2_500); // give the viewer a moment to read the metadata

  // 5. Scroll down to the items + plan section so they're in frame.
  await page.evaluate(() => window.scrollTo({ top: 320, behavior: 'smooth' }));
  await page.waitForTimeout(2_500);

  // 6. Scroll further to plan body.
  await page.evaluate(() => window.scrollTo({ top: 800, behavior: 'smooth' }));
  await page.waitForTimeout(3_000);

  // 7. Scroll back to top.
  await page.evaluate(() => window.scrollTo({ top: 0, behavior: 'smooth' }));
  await page.waitForTimeout(1_500);

  // 8. Back to the list — closes the loop.
  await page.goto('/admin/projects');
  await expect(
    page.locator('tr').filter({ hasText: 'acme-billing-service' }),
  ).toBeVisible();
  await page.waitForTimeout(2_000);
});
