import type { Theme } from '$lib/theme';
import type { TenantContext } from '$lib/types';

declare global {
  namespace App {
    interface Locals {
      tenant: TenantContext;
      theme: Theme;
      /** Tenant has a custom-CSS overlay uploaded — see issue #9. */
      hasCustomCss?: boolean;
    }
    interface PageData {
      tenant: TenantContext;
      theme: Theme;
      /** Mirrors `locals.hasCustomCss` for the root layout to read. */
      hasCustomCss?: boolean;
    }
  }
}

export {};
