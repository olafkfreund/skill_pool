import { fail } from '@sveltejs/kit';
import { getNotifications, putNotifications } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const config = (await getNotifications(auth)) ?? { webhook_url: null, signing_enabled: false };
  return { config };
};

export const actions: Actions = {
  save: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const url = String(data.get('webhook_url') ?? '').trim();
    const secretInput = String(data.get('webhook_secret') ?? '');
    const clearSecret = data.get('clear_secret') === '1';

    // Empty URL clears the webhook. Otherwise the API validates.
    const body: { webhook_url: string | null; webhook_secret?: string | null } = {
      webhook_url: url.length === 0 ? null : url,
    };
    if (clearSecret) {
      body.webhook_secret = ''; // empty string → clear on the server side
    } else if (secretInput.length > 0) {
      body.webhook_secret = secretInput;
    }
    // (no `webhook_secret` key means "leave existing untouched")

    const result = await putNotifications(auth, body);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { saved: true, config: result.config };
  },
};
