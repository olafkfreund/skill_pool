<script lang="ts">
  import { page } from '$app/stores';

  let { children } = $props();

  // Marketplace routes need a full-width layout; login and other public
  // pages keep the original centered card layout.
  const isMarketplace = $derived($page.url.pathname.startsWith('/marketplace'));
</script>

{#if isMarketplace}
  <div class="min-h-screen bg-[var(--sp-bg)]">
    {@render children?.()}
  </div>
{:else}
  <main class="flex min-h-screen items-center justify-center px-4">
    {@render children?.()}
  </main>
{/if}
