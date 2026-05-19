import { fail } from '@sveltejs/kit';
import {
  deleteCustomCss,
  deleteFavicon,
  deleteLogo,
  fetchCustomCss,
  fromClientTheme,
  getFonts,
  getTheme,
  putTheme,
  toClientTheme,
  uploadCustomCss,
  uploadFavicon,
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

/**
 * Favicons accept everything a logo does plus `image/x-icon`. Same allow-list
 * the server enforces — we mirror it here to bail before sending bytes over
 * the wire when the user picks a `.bmp`.
 */
const ALLOWED_FAVICON_MIME = new Set([
  'image/svg+xml',
  'image/png',
  'image/jpeg',
  'image/webp',
  'image/x-icon',
  'image/vnd.microsoft.icon',
]);

/** Same as the server `MAX_LOGO_BYTES` (256 KiB). Mirrored to bail early. */
const MAX_LOGO_BYTES = 256 * 1024;

/** Server-side favicon cap is 64 KiB; mirror it for an early friendly error. */
const MAX_FAVICON_BYTES = 64 * 1024;

/** Server-side custom-CSS cap is 32 KiB; mirror it for early validation. */
const MAX_CUSTOM_CSS_BYTES = 32 * 1024;

/**
 * Fallback allowlist used when /v1/theme/fonts is unreachable at load. Kept
 * in sync with `ALLOWED_FONTS` in `server/src/routes/theme.rs` — the server
 * is authoritative, this is purely a degraded-mode UI affordance.
 */
const FALLBACK_FONT_ALLOWLIST = [
  'system',
  'Inter',
  'IBM Plex Sans',
  'JetBrains Mono',
  'Source Sans 3',
  'Source Serif 4',
  'Merriweather',
  'Roboto',
  'Fira Sans',
  'Atkinson Hyperlegible',
  'Work Sans',
  'Lora',
];

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const server = await getTheme(auth);
  const theme = server
    ? toClientTheme(server)
    : { ...DEFAULT_THEME, brandName: locals.tenant.slug };
  // Logo + favicon presence are encoded as booleans — the bytes are streamed
  // by the `/admin/theme/logo` (and analogous favicon) proxy routes.
  const hasLogo = await checkAssetExists(locals.tenant.slug, 'logo');
  const hasFavicon = await checkAssetExists(locals.tenant.slug, 'favicon');
  // Fetch the curated font allowlist from the server so the picker stays
  // honest about which families are accepted. Fall back to a hard-coded
  // mirror when the server is unreachable.
  const fonts = (await getFonts(auth)) ?? FALLBACK_FONT_ALLOWLIST;
  // Pull the existing CSS overlay (if any) so the textarea is pre-populated.
  // The API returns the raw bytes — we surface them as `customCss` for the
  // editor; empty string means "no overlay set yet".
  let customCss = '';
  try {
    customCss = (await fetchCustomCss(auth)) ?? '';
  } catch {
    // best-effort: render an empty editor rather than blocking page load
  }
  return { theme, hasLogo, hasFavicon, fonts, customCss };
};

async function checkAssetExists(
  tenantSlug: string,
  kind: 'logo' | 'favicon',
): Promise<boolean> {
  // Lightweight presence check against the API server. We use GET because
  // the server doesn't currently expose HEAD; the response is tiny so the
  // overhead is acceptable for a single admin page load.
  const base = process.env.SKILL_POOL_API_BASE?.replace(/\/$/, '') ?? 'http://127.0.0.1:8080';
  try {
    const resp = await fetch(`${base}/v1/theme/${kind}`, {
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

    // Font family: the picker submits `"system"` for the OS stack — we
    // canonicalise that into `undefined` so the server stores NULL and the
    // CSS resolver picks the same default everywhere. Any other value is
    // forwarded as-is; the server validates against `ALLOWED_FONTS`.
    const rawFont = String(data.get('fontFamily') ?? '').trim();
    const fontFamily = rawFont && rawFont !== 'system' ? rawFont : undefined;

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
      fontFamily,
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

  /**
   * Favicon upload. Mirrors the logo action with a smaller cap (64 KiB)
   * and an extended MIME allow-list (`image/x-icon`).
   */
  favicon: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const file = data.get('favicon');
    if (!(file instanceof File) || file.size === 0) {
      return fail(400, { error: 'choose a favicon file before uploading' });
    }
    if (file.size > MAX_FAVICON_BYTES) {
      return fail(400, {
        error: `favicon is ${file.size} bytes; the limit is ${MAX_FAVICON_BYTES} bytes (64 KiB)`,
      });
    }
    if (!ALLOWED_FAVICON_MIME.has(file.type)) {
      return fail(400, {
        error: `unsupported content type "${file.type}" (allowed: SVG, PNG, JPEG, WEBP, ICO)`,
      });
    }
    const result = await uploadFavicon(auth, file);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'favicon upload failed' });
    }
    return { savedFavicon: true, theme: toClientTheme(result.theme), hasFavicon: true };
  },

  removeFavicon: async ({ locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const result = await deleteFavicon(auth);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'favicon delete failed' });
    }
    return { removedFavicon: true, hasFavicon: false };
  },

  /**
   * Save the custom-CSS overlay. The textarea posts as a regular form field
   * (`customCss`) — we wrap it into a `text/css` blob and forward through
   * the API, which runs the deny-rule sanitizer before persisting.
   */
  customCss: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const css = String(data.get('customCss') ?? '');
    if (css.trim().length === 0) {
      return fail(400, {
        error: 'paste a CSS overlay or use Remove to clear the existing one',
      });
    }
    // The server measures bytes after UTF-8 encoding; mirror that here so a
    // pasted payload that's borderline isn't surprising. `Blob.size` returns
    // the byte count of the encoded blob, which is what the API will see.
    const byteLen = new Blob([css]).size;
    if (byteLen > MAX_CUSTOM_CSS_BYTES) {
      return fail(400, {
        error: `custom CSS is ${byteLen} bytes; the limit is ${MAX_CUSTOM_CSS_BYTES} bytes (32 KiB)`,
      });
    }
    const result = await uploadCustomCss(auth, css);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'custom CSS upload failed' });
    }
    return { savedCustomCss: true, customCss: css };
  },

  removeCustomCss: async ({ locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const result = await deleteCustomCss(auth);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'custom CSS delete failed' });
    }
    return { removedCustomCss: true, customCss: '' };
  },
};
