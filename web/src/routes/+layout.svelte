<script lang="ts">
  import '../app.css';
  import { themeToCss } from '$lib/theme';

  let { data, children } = $props();

  const styleBlock = $derived(`:root { ${themeToCss(data.theme)} }`);
</script>

<svelte:head>
  <title>{data.theme.brandName} · skill-pool</title>
  {@html `<style>${styleBlock}</style>`}
</svelte:head>

{@render children?.()}
