import { fail } from '@sveltejs/kit';
import {
  ApiError,
  createMyToken,
  listMyTokens,
  revokeMyToken,
  whoami,
  type ApiToken,
  type CreatedApiToken,
  type WhoAmI,
} from '$lib/server/api';
import type { Actions, PageServerLoad } from './$types';

const ALLOWED_SCOPES = ['skills:read', 'skills:publish', 'tenant:admin'] as const;
type Scope = (typeof ALLOWED_SCOPES)[number];

function parseAllScopes(values: FormDataEntryValue[]): Scope[] {
  const out: Scope[] = [];
  for (const v of values) {
    const s = String(v);
    if ((ALLOWED_SCOPES as readonly string[]).includes(s)) {
      out.push(s as Scope);
    }
  }
  return Array.from(new Set(out));
}

export const load: PageServerLoad = async ({ locals, cookies }) => {
  const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };

  let identity: WhoAmI | null = null;
  let tokens: ApiToken[] = [];
  let loadError: string | undefined;

  try {
    identity = await whoami(auth);
  } catch {
    // whoami returns null on 4xx already; only network errors land here.
    identity = null;
  }

  try {
    tokens = await listMyTokens(auth);
  } catch (e) {
    if (e instanceof ApiError) {
      // Pure API-token callers can't list (401). Surface a friendly
      // explanation rather than the raw 401 body.
      loadError =
        e.status === 401
          ? 'You must be signed in through SSO to manage personal API tokens.'
          : `Could not load tokens: ${e.message}`;
    } else {
      loadError = 'Could not load tokens.';
    }
  }

  return { identity, tokens, loadError };
};

export const actions: Actions = {
  create: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();

    const label = String(data.get('label') ?? '').trim();
    const scopes = parseAllScopes(data.getAll('scopes'));

    if (!label) {
      return fail(400, { error: 'Label is required.' });
    }
    if (label.length > 80) {
      return fail(400, { error: 'Label must be 80 characters or fewer.' });
    }
    if (scopes.length === 0) {
      return fail(400, { error: 'Select at least one scope.' });
    }

    const result = await createMyToken(auth, label, scopes);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    // Token shown ONCE. The UI surfaces it in a modal then forgets it.
    const created: CreatedApiToken = result.token;
    return { created };
  },

  revoke: async ({ request, locals, cookies }) => {
    const auth = { tenant: locals.tenant.slug, token: cookies.get('sp_token') };
    const data = await request.formData();
    const id = String(data.get('id') ?? '').trim();
    if (!id) {
      return fail(400, { error: 'Token id is required.' });
    }
    const result = await revokeMyToken(auth, id);
    if (!result.ok) {
      return fail(result.status, { error: result.error });
    }
    return { revoked: true, id };
  },
};

// Re-export for the +page.svelte component so it can keep types tight
// without re-importing from `$lib/server/api` (the latter is server-only).
export type { ApiToken, CreatedApiToken, WhoAmI };
