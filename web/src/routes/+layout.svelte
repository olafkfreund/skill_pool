<script lang="ts">
  import '../app.css';
  import { themeToCss } from '$lib/theme';

  let { data, children } = $props();

  // The `:root` block carries `--sp-font-family` (resolved by `themeToCss`).
  // The `body` rule below pulls the variable into the actual cascade so the
  // tenant's font choice applies to every page without each component opting
  // in. The `system-ui` fallback covers the bootstrap moment before the
  // variable has been parsed.
  const styleBlock = $derived(
    `:root { ${themeToCss(data.theme)} } body { font-family: var(--sp-font-family, system-ui); }`,
  );

  // For non-system fonts, inject the Google Fonts stylesheet so the chosen
  // family actually renders. We never inject the stylesheet for the system
  // stack — that would add a useless network round-trip.
  const googleFontUrl = $derived(
    data.theme.fontFamily && data.theme.fontFamily !== 'system'
      ? `https://fonts.googleapis.com/css2?family=${encodeURIComponent(data.theme.fontFamily).replace(/%20/g, '+')}:wght@400;500;600;700&display=swap`
      : null,
  );
</script>

<svelte:head>
  <title>{data.theme.brandName} · skill-pool</title>
  {@html `<style>${styleBlock}</style>`}
  {#if googleFontUrl}
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous" />
    <link rel="stylesheet" href={googleFontUrl} />
  {/if}
  <!-- Tenant-uploaded CSS overlay (#9). Pulled from the proxy route so the
       API server can resolve the tenant by host. The endpoint serves bytes
       with `Content-Security-Policy: style-src 'self'` as defence-in-depth
       under the sanitizer. Cached for 5 minutes upstream. -->
  {#if data.hasCustomCss}
    <link rel="stylesheet" href="/theme/custom.css" />
  {/if}
</svelte:head>

{@render children?.()}
