// Public (non-server) module so `+page.svelte` can import these
// symbols without violating SvelteKit's `$lib/server/*` boundary.
// `$lib/server/api.ts` re-exports them for convenience so existing
// server-side callers keep working unchanged.

export type SsoRole = 'viewer' | 'publisher' | 'curator' | 'admin';

export const SSO_ROLES: SsoRole[] = ['viewer', 'publisher', 'curator', 'admin'];
