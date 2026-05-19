import { fail, redirect } from '@sveltejs/kit';
import {
  discoverOidc,
  discoverSaml,
  getSessionMaxAge,
  oidcStartUrl,
  samlMetadataUrl,
  validateAuth,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

export const load: PageServerLoad = async ({ locals, url }) => {
  if (locals.tenant.authed) {
    throw redirect(303, url.searchParams.get('next') ?? '/');
  }

  const auth = { tenant: locals.tenant.slug };
  const [oidc, saml] = await Promise.all([discoverOidc(auth), discoverSaml(auth)]);
  const returnTo = `${url.origin}/oidc-return`;

  return {
    sso: {
      oidc,
      saml,
      anyEnabled: oidc.enabled || saml.enabled,
    },
    oidcStart: oidc.enabled ? oidcStartUrl(locals.tenant.slug, returnTo) : null,
    samlMetadataUrl: saml.enabled ? samlMetadataUrl(locals.tenant.slug) : null,
  };
};

export const actions: Actions = {
  default: async ({ request, cookies, locals, url }) => {
    const data = await request.formData();
    const token = (data.get('token') ?? '').toString().trim();

    if (!token) {
      return fail(400, { error: 'token is required' });
    }

    const ok = await validateAuth({ tenant: locals.tenant.slug, token });
    if (!ok) {
      return fail(401, { error: 'token rejected by registry' });
    }

    // Per-tenant idle-timeout policy: a tenant may have a stricter
    // session lifetime than the 14-day default (e.g. 1 hour for
    // regulated workloads). getSessionMaxAge falls back to 14 days on
    // any error so the login never blocks on the policy fetch.
    const maxAge = await getSessionMaxAge(locals.tenant.slug);
    cookies.set('sp_token', token, {
      path: '/',
      httpOnly: true,
      sameSite: 'lax',
      secure: url.protocol === 'https:',
      maxAge,
    });
    cookies.set('sp_tenant', locals.tenant.slug, {
      path: '/',
      httpOnly: true,
      sameSite: 'lax',
      secure: url.protocol === 'https:',
      maxAge,
    });

    throw redirect(303, url.searchParams.get('next') ?? '/');
  },
};
