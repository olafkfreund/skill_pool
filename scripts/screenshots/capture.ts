// Playwright-driven screenshot capture for the skill-pool portal.
//
// Reads `pages.json` (a list of {name, url, waitForSelector?, fullPage?,
// skipAuth?, viewport?} entries), boots a headless Chromium pinned at the
// Nix-provided binary via `PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH`, sets the
// portal's auth cookies (`sp_token` + `sp_tenant`), navigates each route,
// and writes a deduped WebP into `docs/images/`.
//
// Usage (from repo root):
//   cd scripts/screenshots
//   npm install --no-audit --no-fund
//   SP_TOKEN=spk_… node --import tsx capture.ts
//
// Env:
//   SP_TOKEN   (required) — bearer token, raw `sp_…` value
//   SP_TENANT  (default acme) — tenant slug, also used in the sp_tenant cookie
//   SP_BASE    (default http://127.0.0.1:3000) — portal origin
//   PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH — set by the Nix devShell

import { chromium, type Browser, type BrowserContext } from 'playwright';
import * as fs from 'node:fs';
import * as path from 'node:path';
import * as os from 'node:os';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const __dirname = path.dirname(fileURLToPath(import.meta.url));

interface PageSpec {
  name: string;
  url: string;
  waitForSelector?: string;
  beforeScreenshot?: string; // serialized JS to evaluate in the page
  viewport?: { width: number; height: number };
  fullPage?: boolean;
  skipAuth?: boolean;
  // Capture the SSR markup without running client-side JS. Use for routes
  // whose client hydration is broken in the dev server (e.g. SvelteKit's
  // `$lib/server/*` import boundary check trips a known dev-only error).
  noJs?: boolean;
}

const TOKEN = process.env.SP_TOKEN;
const TENANT = process.env.SP_TENANT ?? 'acme';
const BASE = process.env.SP_BASE ?? 'http://127.0.0.1:3000';

if (!TOKEN) {
  console.error('SP_TOKEN env var required (e.g. SP_TOKEN=spk_… node --import tsx capture.ts)');
  process.exit(1);
}

const baseUrl = new URL(BASE);
const cookieDomain = baseUrl.hostname;

const pagesPath = path.join(__dirname, 'pages.json');
const pages: PageSpec[] = JSON.parse(fs.readFileSync(pagesPath, 'utf-8'));

const outDir = path.resolve(__dirname, '../../docs/images');
fs.mkdirSync(outDir, { recursive: true });

const execPath = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;
if (!execPath) {
  console.warn('PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH unset — falling back to Playwright bundled Chromium (likely fails on Nix).');
}

const launchOpts: Parameters<typeof chromium.launch>[0] = {
  // Pass the Nix Chromium when present; fall through to bundled otherwise.
  ...(execPath ? { executablePath: execPath } : {}),
  // Modern Chromium on Nix needs the no-sandbox dance because the multi-user
  // sandbox helper isn't shipped with the Nix derivation.
  args: ['--no-sandbox', '--disable-dev-shm-usage'],
};

async function run(): Promise<void> {
  const browser: Browser = await chromium.launch(launchOpts);
  const context: BrowserContext = await browser.newContext({
    viewport: { width: 1440, height: 900 },
    deviceScaleFactor: 2,
    // The portal renders OG meta + nav using these.
    userAgent: 'Mozilla/5.0 (X11; Linux x86_64) skill-pool-screenshot/0.1',
  });
  await context.addCookies([
    { name: 'sp_token', value: TOKEN!, domain: cookieDomain, path: '/', httpOnly: true },
    { name: 'sp_tenant', value: TENANT, domain: cookieDomain, path: '/', httpOnly: true },
  ]);

  const results: { name: string; bytes: number; ok: boolean; note?: string }[] = [];

  for (const p of pages) {
    // Per-page context so we can independently toggle JS, cookies, etc.
    const pageCtx = p.noJs
      ? await browser.newContext({
          viewport: { width: 1440, height: 900 },
          deviceScaleFactor: 2,
          javaScriptEnabled: false,
          userAgent: 'Mozilla/5.0 (X11; Linux x86_64) skill-pool-screenshot/0.1',
        })
      : context;
    if (p.noJs && !p.skipAuth) {
      await pageCtx.addCookies([
        { name: 'sp_token', value: TOKEN!, domain: cookieDomain, path: '/', httpOnly: true },
        { name: 'sp_tenant', value: TENANT, domain: cookieDomain, path: '/', httpOnly: true },
      ]);
    }
    const page = await pageCtx.newPage();
    if (p.viewport) {
      await page.setViewportSize(p.viewport);
    }
    const target = `${BASE}${p.url}`;
    try {
      // For pages that should *not* be authed (login), drop the cookies first.
      if (p.skipAuth && !p.noJs) {
        await context.clearCookies();
      }

      await page.goto(target, { waitUntil: p.noJs ? 'load' : 'networkidle', timeout: 20_000 });

      // Vite's dep-optimizer occasionally serves a stale chunk on the first
      // cold load of a route (manifests as a 500 on a generated .js node and
      // an empty <body>). The HMR error overlay then takes over the page.
      // Detect either condition and bounce the page until we get real content,
      // up to 3 attempts. Skipped for `noJs:true` routes where we want the
      // raw SSR markup.
      for (let attempt = 0; attempt < 3 && !p.noJs; attempt++) {
        const state = await page.evaluate(() => ({
          overlay: document.querySelectorAll('vite-error-overlay').length,
          bodyLen: document.body ? document.body.innerText.length : 0,
        }));
        if (state.overlay === 0 && state.bodyLen > 80) break;
        console.log(`    retry ${p.name} attempt=${attempt + 1} overlay=${state.overlay} body=${state.bodyLen}`);
        await page.waitForTimeout(2500);
        try {
          await page.reload({ waitUntil: 'networkidle', timeout: 20_000 });
        } catch (e) {
          // Surface but keep going
          console.log(`    reload error: ${e instanceof Error ? e.message : e}`);
        }
      }

      // If we landed back on /login while expecting an authed view, that's
      // an auth failure — flag it but still take the shot so debugging is easier.
      const finalUrl = page.url();
      let authNote: string | undefined;
      if (!p.skipAuth && /\/login(\?|$)/.test(finalUrl)) {
        authNote = `redirected to login (final=${finalUrl})`;
      }

      if (p.waitForSelector) {
        await page.waitForSelector(p.waitForSelector, { timeout: 5_000 }).catch(() => {});
      }

      if (p.beforeScreenshot) {
        // eslint-disable-next-line @typescript-eslint/no-implied-eval
        await page.evaluate(p.beforeScreenshot);
      }

      // Belt-and-braces: nuke any straggling Vite error overlay so it can't
      // bleed into the screenshot if it re-appeared after reload.
      await page.evaluate(() => {
        document.querySelectorAll('vite-error-overlay').forEach((el) => el.remove());
      });

      // Settle for fonts + animations + skeleton loaders.
      await page.waitForTimeout(800);

      // Playwright's native screenshot encoder only emits PNG/JPEG. We want
      // WebP for size+quality at parity, so capture as PNG to a tempfile and
      // then transcode with ImageMagick (`magick`) which is in the devShell.
      const outFile = path.join(outDir, `${p.name}.webp`);
      const tmpPng = path.join(os.tmpdir(), `skill-pool-${p.name}-${process.pid}.png`);
      await page.screenshot({
        path: tmpPng,
        type: 'png',
        fullPage: !!p.fullPage,
      });
      try {
        execFileSync('magick', [tmpPng, '-quality', '92', '-define', 'webp:method=6', outFile], {
          stdio: ['ignore', 'ignore', 'inherit'],
        });
      } finally {
        fs.rmSync(tmpPng, { force: true });
      }

      const stat = fs.statSync(outFile);
      const sizeKb = (stat.size / 1024).toFixed(1);
      results.push({ name: p.name, bytes: stat.size, ok: !authNote, note: authNote });
      console.log(`  ${authNote ? '!' : 'ok'} ${p.name.padEnd(14)} ${sizeKb.padStart(7)} KiB  ${target}${authNote ? `  [${authNote}]` : ''}`);

      // Restore cookies if we cleared them for a public page.
      if (p.skipAuth) {
        await context.addCookies([
          { name: 'sp_token', value: TOKEN!, domain: cookieDomain, path: '/', httpOnly: true },
          { name: 'sp_tenant', value: TENANT, domain: cookieDomain, path: '/', httpOnly: true },
        ]);
      }
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err);
      results.push({ name: p.name, bytes: 0, ok: false, note: msg });
      console.error(`  FAIL ${p.name.padEnd(14)} ${target}  ${msg}`);
    } finally {
      await page.close();
      if (p.noJs) {
        await pageCtx.close();
      }
    }
  }

  await browser.close();

  console.log('\nSummary:');
  for (const r of results) {
    const marker = r.ok ? 'ok  ' : 'WARN';
    const size = r.bytes ? `${(r.bytes / 1024).toFixed(1)} KiB` : 'no-file';
    console.log(`  ${marker} ${r.name.padEnd(14)} ${size}${r.note ? `  (${r.note})` : ''}`);
  }

  const failed = results.filter((r) => !r.ok).length;
  if (failed > 0) {
    console.error(`\n${failed} page(s) reported issues — see [bracketed] notes above.`);
    process.exitCode = 2;
  }
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
