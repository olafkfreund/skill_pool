import { fail } from '@sveltejs/kit';
import {
  deleteLogo,
  fromClientTheme,
  getTheme,
  putTheme,
  toClientTheme,
  uploadLogo,
} from '$lib/server/api';
import { DEFAULT_THEME, type Theme } from '$lib/theme';
import { checkThemeContrast } from '$lib/contrast';
import type { Actions, PageServerLoad } from './$types';

/** Allowed client-side MIME types — mirrors the server allow-list. */
const ALLOWED_LOGO_MIME = new Set([
  'image/svg+xml',
  'image/png',
  'image/jpeg',
  'image/webp',
]);

/** Same as the server `MAX_LOGO_BYTES` (256 KiB). Mirrored to bail early. */
const MAX_LOGO_BYTES = 256 * 1024;

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const server = await getTheme(auth);
  const theme = server
    ? toClientTheme(server)
    : { ...DEFAULT_THEME, brandName: locals.tenant.slug };
  // Logo presence is encoded as a boolean — the bytes are streamed by the
  // `/admin/theme/logo` SvelteKit proxy route, not embedded inline.
  const hasLogo = await checkLogoExists(locals.tenant.slug);
  return { theme, hasLogo };
};

async function checkLogoExists(tenantSlug: string): Promise<boolean> {
  // Lightweight HEAD-equivalent against the API server. We use GET because
  // the server doesn't currently expose HEAD; the response is tiny so the
  // overhead is acceptable for a single admin page load.
  const base = process.env.SKILL_POOL_API_BASE?.replace(/\/$/, '') ?? 'http://127.0.0.1:8080';
  try {
    const resp = await fetch(`${base}/v1/theme/logo`, {
      method: 'GET',
      headers: { 'X-Skill-Pool-Tenant': tenantSlug },
    });
    return resp.ok;
  } catch {
    return false;
  }
}

export const actions: Actions = {
  save: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const theme: Theme = {
      brandName: String(data.get('brandName') ?? '').trim(),
      primary: String(data.get('primary') ?? '').trim(),
      primaryFg: String(data.get('primaryFg') ?? '').trim(),
      accent: String(data.get('accent') ?? '').trim(),
      bg: String(data.get('bg') ?? '').trim(),
      fg: String(data.get('fg') ?? '').trim(),
      muted: String(data.get('muted') ?? '').trim(),
      mutedFg: String(data.get('mutedFg') ?? '').trim(),
      border: String(data.get('border') ?? '').trim(),
      radius: String(data.get('radius') ?? '0.5rem').trim(),
      // Checkbox: present = true, absent = false.
      footerBranding: data.has('footerBranding'),
    };

    // WCAG AA contrast validation — refuse to save before touching the API.
    const contrastFailures = checkThemeContrast(theme);
    if (contrastFailures.length > 0) {
      const lines = contrastFailures.map(
        (f) => `${f.pair}: ${f.ratio} (need ${f.required})`,
      );
      return fail(422, {
        error: `WCAG AA contrast failures:\n${lines.join('\n')}`,
        contrastFailures,
        draft: theme,
      });
    }

    const result = await putTheme(auth, fromClientTheme(theme));
    if (!result.ok) {
      return fail(result.status, {
        error: result.error ?? 'save failed',
        contrastFailures: [] as ReturnType<typeof checkThemeContrast>,
        draft: theme,
      });
    }
    return { saved: true, theme };
  },

  reset: async ({ locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const defaults: Theme = { ...DEFAULT_THEME, brandName: locals.tenant.slug };
    const result = await putTheme(auth, fromClientTheme(defaults));
    if (!result.ok) {
      return fail(result.status, {
        error: result.error ?? 'reset failed',
        contrastFailures: [] as ReturnType<typeof checkThemeContrast>,
        draft: defaults,
      });
    }
    return { saved: true, theme: defaults };
  },

  // Logo upload (multipart). The server enforces SVG sanitization + a 256
  // KiB cap; we replicate the cap and the MIME allow-list here purely to
  // give a friendlier early error before sending bytes over the wire.
  logo: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const file = data.get('logo');
    if (!(file instanceof File) || file.size === 0) {
      return fail(400, { error: 'choose a logo file before uploading' });
    }
    if (file.size > MAX_LOGO_BYTES) {
      return fail(400, {
        error: `logo is ${file.size} bytes; the limit is ${MAX_LOGO_BYTES} bytes (256 KiB)`,
      });
    }
    if (!ALLOWED_LOGO_MIME.has(file.type)) {
      return fail(400, {
        error: `unsupported content type "${file.type}" (allowed: SVG, PNG, JPEG, WEBP)`,
      });
    }
    const result = await uploadLogo(auth, file);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'logo upload failed' });
    }
    return { savedLogo: true, theme: toClientTheme(result.theme), hasLogo: true };
  },

  removeLogo: async ({ locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const result = await deleteLogo(auth);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'logo delete failed' });
    }
    return { removedLogo: true, hasLogo: false };
  },
};
