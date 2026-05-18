import { error, redirect } from '@sveltejs/kit';
import { validateAuth } from '$lib/server/api';
import type { PageServerLoad } from './$types';

/**
 * Handles the server's redirect back from the OIDC callback.
 *
 *   GET /oidc-return?token=sps_...&tenant=acme
 *
 * Validates the token by hitting the API (proves the session is real),
 * sets the cookies the rest of the portal already relies on, redirects
 * to the catalog. Refuses if the tenant in the query doesn't match the
 * tenant we resolved from the request host — that prevents an attacker
 * from luring `acme` users into a flow that drops them on `globex`.
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
