<script lang="ts">
  let { data } = $props();
</script>

<header class="mb-8">
  <h1 class="text-2xl font-semibold">Theme</h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Brand colours, logo, and custom domain. Wiring to the server's <code
      class="rounded bg-[var(--sp-muted)] px-1">tenant_theme</code
    > table lands in #9.
  </p>
</header>

<div
  class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-8 text-sm text-[var(--sp-muted-fg)]"
>
  <p class="mb-4">Current theme (read-only preview):</p>
  <dl class="grid grid-cols-1 gap-3 sm:grid-cols-2">
    {#each Object.entries(data.theme) as [key, value] (key)}
      <div class="flex items-center justify-between rounded-md bg-[var(--sp-muted)] px-3 py-2">
        <dt class="font-mono text-xs">{key}</dt>
        <dd class="flex items-center gap-2 text-xs">
          {#if typeof value === 'string' && value.startsWith('#')}
            <span
              class="inline-block h-4 w-4 rounded border border-[var(--sp-border)]"
              style="background: {value};"
            ></span>
          {/if}
          <code>{value}</code>
        </dd>
      </div>
    {/each}
  </dl>
</div>
