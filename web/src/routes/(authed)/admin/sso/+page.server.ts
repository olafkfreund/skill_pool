import { fail } from '@sveltejs/kit';
import {
  deleteSsoConfig,
  discoverOidc,
  discoverSaml,
  getSsoConfig,
  putSsoOidc,
  putSsoSaml,
  samlMetadataUrl,
  SSO_ROLES,
  type SsoConfigView,
  type SsoRole,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

const DEFAULT_CONFIG: SsoConfigView = {
  kind: null,
  scim_endpoint: '/scim/v2/Users',
};

function isRole(v: string): v is SsoRole {
  return (SSO_ROLES as readonly string[]).includes(v);
}

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
  const config = (await getSsoConfig(auth)) ?? DEFAULT_CONFIG;
  return {
    config,
    samlMetadataUrl: samlMetadataUrl(locals.tenant.slug),
  };
};

export const actions: Actions = {
  /** Upsert the OIDC config for this tenant. */
  saveOidc: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const issuer_url = String(data.get('issuer_url') ?? '').trim();
    const client_id = String(data.get('client_id') ?? '').trim();
    const client_secret = String(data.get('client_secret') ?? '');
    const role = String(data.get('default_role') ?? 'viewer');

    if (!issuer_url || !client_id || !client_secret) {
      return fail(400, {
        error: 'issuer_url, client_id, and client_secret are all required',
      });
    }
    if (!isRole(role)) {
      return fail(400, { error: `invalid role \`${role}\`` });
    }

    const result = await putSsoOidc(auth, {
      issuer_url,
      client_id,
      client_secret,
      default_role: role,
    });
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { saved: 'oidc' as const, config: result.config };
  },

  /** Upsert the SAML config from pasted IdP metadata XML. */
  saveSaml: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const metadata_xml = String(data.get('metadata_xml') ?? '').trim();
    const role = String(data.get('default_role') ?? 'viewer');
    const sp_entity_id = String(data.get('sp_entity_id') ?? '').trim();

    if (!metadata_xml) {
      return fail(400, { error: 'metadata_xml is required' });
    }
    if (!isRole(role)) {
      return fail(400, { error: `invalid role \`${role}\`` });
    }

    const result = await putSsoSaml(auth, {
      metadata_xml,
      default_role: role,
      sp_entity_id: sp_entity_id || null,
    });
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { saved: 'saml' as const, config: result.config };
  },

  /** Clear both OIDC and SAML rows. */
  clear: async ({ locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const result = await deleteSsoConfig(auth);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { cleared: true as const, config: DEFAULT_CONFIG };
  },

  /**
   * Probe the runtime discovery endpoint. Reports whether the IdP is
   * reachable from the server side — different from "config is saved",
   * which `getSsoConfig` already covers.
   */
  test: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const which = String(data.get('which') ?? 'oidc');
    try {
      if (which === 'saml') {
        const r = await discoverSaml(auth);
        return r.enabled
          ? { tested: 'saml' as const, ok: true as const }
          : {
              tested: 'saml' as const,
              ok: false as const,
              error: 'SAML not configured for this tenant yet',
            };
      }
      const r = await discoverOidc(auth);
      return r.enabled
        ? { tested: 'oidc' as const, ok: true as const }
        : {
            tested: 'oidc' as const,
            ok: false as const,
            error: 'OIDC not configured for this tenant yet',
          };
    } catch (e) {
      return {
        tested: which as 'oidc' | 'saml',
        ok: false as const,
        error: e instanceof Error ? e.message : String(e),
      };
    }
  },
};
