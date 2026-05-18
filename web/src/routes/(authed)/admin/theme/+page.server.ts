import { fail } from '@sveltejs/kit';
import { fromClientTheme, getTheme, putTheme, toClientTheme } from '$lib/server/api';
import { DEFAULT_THEME, type Theme } from '$lib/theme';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const server = await getTheme(auth);
  const theme = server
    ? toClientTheme(server)
    : { ...DEFAULT_THEME, brandName: locals.tenant.slug };
  return { theme };
};

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
    };

    const result = await putTheme(auth, fromClientTheme(theme));
    if (!result.ok) {
      return fail(result.status, {
        error: result.error ?? 'save failed',
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
        draft: defaults,
      });
    }
    return { saved: true, theme: defaults };
  },
};
