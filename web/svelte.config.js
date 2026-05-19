import adapter from '@sveltejs/adapter-node';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

/** @type {import('@sveltejs/kit').Config} */
const config = {
  preprocess: vitePreprocess(),

  kit: {
    adapter: adapter(),
    // CSRF Origin allowlist (lives at `kit.csrf.trustedOrigins`; the
    // top-level `csrf` block is silently ignored).
    //
    // SvelteKit always trusts the request whose Origin matches `url.origin`
    // (i.e. the ORIGIN env var). This list adds extra Origins that are
    // also accepted on POST/PUT/PATCH/DELETE.
    //
    // `'*'` means "accept any Origin" — equivalent to disabling the check
    // entirely. Use it when the same instance is reached over many host
    // names (localhost + LAN IP + .lan/.local mDNS + a *.nip.io URL).
    //
    // Why this is safe enough for skill-pool: CSRF defence still has
    //   * `sp_token` / `sp_tenant` cookies — httpOnly + sameSite: 'lax'
    //     (browsers don't send them on cross-site POSTs from third-party
    //     origins)
    //   * server-side bearer-token auth on every mutating endpoint
    //   * `secure` flag tied to https://
    // The Origin check is belt-and-braces on top of those.
    //
    // For a public internet deploy on one canonical domain, drop the
    // wildcard and list specific Origins or pin a single ORIGIN env.
    csrf: {
      trustedOrigins: [
        '*',
        // Permanent overrides take precedence over `*` matching. Add
        // specific Origins here only if you switch off the wildcard.
        ...(process.env.SP_TRUSTED_ORIGINS?.split(',').map((s) => s.trim()).filter(Boolean) ?? []),
      ],
    },
  },
};

export default config;
