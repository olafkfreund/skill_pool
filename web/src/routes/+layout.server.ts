import type { LayoutServerLoad } from './$types';

export const load: LayoutServerLoad = async ({ locals }) => {
  return {
    tenant: locals.tenant,
    theme: locals.theme,
    // Whether the tenant has a custom-CSS overlay set. The bytes themselves
    // are served by `/admin/theme/custom-css` (proxied to the API). We expose
    // a boolean here so the root layout can conditionally inject the
    // `<link rel="stylesheet">` without a second round trip on every page.
    hasCustomCss: locals.hasCustomCss ?? false,
  };
};
