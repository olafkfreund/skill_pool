import { fail } from '@sveltejs/kit';
import { getNotifications, putNotifications, type PutNotificationsBody } from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const config = (await getNotifications(auth)) ?? { webhook_url: null, signing_enabled: false };
  return { config };
};

export const actions: Actions = {
  /** Webhook section. Body is unchanged from the original form. */
  saveWebhook: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const url = String(data.get('webhook_url') ?? '').trim();
    const secretInput = String(data.get('webhook_secret') ?? '');
    const clearSecret = data.get('clear_secret') === '1';

    const body: PutNotificationsBody = {
      webhook_url: url.length === 0 ? null : url,
    };
    if (clearSecret) {
      body.webhook_secret = '';
    } else if (secretInput.length > 0) {
      body.webhook_secret = secretInput;
    }

    const result = await putNotifications(auth, body);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { saved: 'webhook', config: result.config };
  },

  /** Email (SMTP) section. Partial-update via the same endpoint. */
  saveEmail: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const smtp_url = String(data.get('smtp_url') ?? '').trim();
    const smtp_from = String(data.get('smtp_from') ?? '').trim();
    const smtp_to = String(data.get('smtp_to') ?? '').trim();

    const body: PutNotificationsBody = {
      smtp_url: smtp_url.length === 0 ? null : smtp_url,
      smtp_from: smtp_from.length === 0 ? null : smtp_from,
      smtp_to: smtp_to.length === 0 ? null : smtp_to,
    };
    const result = await putNotifications(auth, body);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { saved: 'email', config: result.config };
  },
};
