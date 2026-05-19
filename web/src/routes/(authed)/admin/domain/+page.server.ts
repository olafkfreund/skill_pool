import { fail } from '@sveltejs/kit';
import { env } from '$env/dynamic/private';
import {
  ApiError,
  createCustomDomain,
  listCustomDomains,
  removeCustomDomain,
  verifyCustomDomain,
  type CustomDomain,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

/**
 * Fallback CNAME target shown when the deploy didn't bake a public hostname
 * into the env. Matches the example in `docs/enterprise/custom-domains.md` so
 * the UI is at least self-explanatory during local dev — production deploys
 * should always set `SKILL_POOL_PUBLIC_HOSTNAME` so the displayed target is
 * the real proxy front-door.
 */
const DEFAULT_PUBLIC_HOSTNAME = 'skill-pool.example.com';

function resolvePublicHostname(): string {
  return env.SKILL_POOL_PUBLIC_HOSTNAME?.trim() || DEFAULT_PUBLIC_HOSTNAME;
}

/**
 * `<tenant>.<public-host>` is the canonical CNAME target an admin points
 * their custom hostname at. The reverse proxy already serves that wildcard
 * cert path, so a CNAME there picks up the per-tenant TLS termination
 * without any further DNS plumbing on our side.
 */
function cnameTargetFor(tenantSlug: string): string {
  return `${tenantSlug}.${resolvePublicHostname()}`;
}

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const cnameTarget = cnameTargetFor(locals.tenant.slug);
  // Whether the deploy set a real public hostname or we're falling back to
  // the docs placeholder. The UI uses this to surface a "ask your admin"
  // help-line instead of silently displaying `skill-pool.example.com` as if
  // it were the real target.
  const cnameTargetIsDefault = !env.SKILL_POOL_PUBLIC_HOSTNAME?.trim();

  try {
    const domains = await listCustomDomains(auth);
    return { domains, cnameTarget, cnameTargetIsDefault };
  } catch (e) {
    if (e instanceof ApiError) {
      return {
        domains: [] as CustomDomain[],
        cnameTarget,
        cnameTargetIsDefault,
        error: `Could not load custom domains: ${e.message || `HTTP ${e.status}`}`,
      };
    }
    return {
      domains: [] as CustomDomain[],
      cnameTarget,
      cnameTargetIsDefault,
      error: 'Could not load custom domains.',
    };
  }
};

export const actions: Actions = {
  /** Claim a hostname. The server validates shape + uniqueness. */
  add: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const hostname = String(data.get('hostname') ?? '').trim();
    if (!hostname) {
      return fail(400, { error: 'Hostname is required.' });
    }
    const result = await createCustomDomain(auth, hostname);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'Could not add domain.' });
    }
    return { added: true, domain: result.domain };
  },

  /**
   * Force the DNS-TXT check now. Always returns the fresh row so the UI can
   * re-render with the new `last_checked_at` / `last_error`. We avoid
   * surfacing a generic toast when the server attaches a specific error —
   * the per-row inline error covers that.
   */
  verify: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '');
    if (!id) {
      return fail(400, { error: 'id is required.' });
    }
    const result = await verifyCustomDomain(auth, id);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'Verify failed.' });
    }
    // The server returns the row in both pass and fail cases — `status` and
    // `last_error` tell us which. Surface the fail inline rather than
    // dressing it up as a "Verify failed" red toast that would obscure the
    // actual resolver message.
    const verified = result.domain.status === 'verified' || result.domain.status === 'active';
    return { verified, domain: result.domain };
  },

  remove: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '');
    if (!id) {
      return fail(400, { error: 'id is required.' });
    }
    const result = await removeCustomDomain(auth, id);
    if (!result.ok) {
      return fail(result.status, { error: result.error || 'Could not remove domain.' });
    }
    return { removed: true, id };
  },
};
