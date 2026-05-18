import { error, redirect } from '@sveltejs/kit';
import { validateAuth } from '$lib/server/api';
import type { PageServerLoad } from './$types';

/**
 * Handles the server's redirect back from /v1/auth/saml/{tenant}/acs after a
 * successful assertion validation. Same contract as /oidc-return.
 */
export const load: PageServerLoad = async ({ url, cookies, locals }) => {
  const token = url.searchParams.get('token');
  const tenant = url.searchParams.get('tenant');

  if (!token || !tenant) {
    throw error(400, 'missing token or tenant in callback');
  }
  if (tenant !== locals.tenant.slug) {
    throw error(400, `tenant mismatch (got ${tenant}, expected ${locals.tenant.slug})`);
  }

  const ok = await validateAuth({ tenant, token });
  if (!ok) {
    throw error(401, 'session token rejected by registry');
  }

  cookies.set('sp_token', token, {
    path: '/',
    httpOnly: true,
    sameSite: 'lax',
    secure: url.protocol === 'https:',
    maxAge: 60 * 60 * 24 * 14,
  });
  cookies.set('sp_tenant', tenant, {
    path: '/',
    httpOnly: true,
    sameSite: 'lax',
    secure: url.protocol === 'https:',
    maxAge: 60 * 60 * 24 * 14,
  });

  throw redirect(303, '/');
};
