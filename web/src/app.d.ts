import type { Theme } from '$lib/theme';
import type { TenantContext } from '$lib/types';

declare global {
  namespace App {
    interface Locals {
      tenant: TenantContext;
      theme: Theme;
    }
    interface PageData {
      tenant: TenantContext;
      theme: Theme;
    }
  }
}

export {};
