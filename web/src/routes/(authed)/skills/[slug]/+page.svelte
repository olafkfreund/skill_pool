<script lang="ts">
  import { ArrowLeft, Download, FileCode } from '@lucide/svelte';
  import MonacoViewer from '$lib/MonacoViewer.svelte';

  let { data } = $props();
</script>

<a
  href="/"
  class="mb-6 inline-flex items-center gap-1 text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
>
  <ArrowLeft size="14" /> Catalog
</a>

<header class="mb-8">
  <div class="flex flex-wrap items-baseline gap-3">
    <h1 class="text-3xl font-semibold">{data.skill.slug}</h1>
    <span class="text-sm text-[var(--sp-muted-fg)]">v{data.skill.version}</span>
    <span
      class="rounded-full px-2 py-0.5 text-xs"
      style="background: var(--sp-muted); color: var(--sp-muted-fg);"
    >
      {data.skill.status}
    </span>
  </div>
  <p class="mt-3 max-w-prose text-[var(--sp-fg)]">{data.skill.description}</p>
  {#if data.skill.tags.length > 0}
    <div class="mt-4 flex flex-wrap gap-1">
      {#each data.skill.tags as tag (tag)}
        <span
          class="rounded-full px-2 py-0.5 text-xs"
          style="background: var(--sp-muted); color: var(--sp-fg); border: 1px solid var(--sp-border);"
          >{tag}</span
        >
      {/each}
    </div>
  {/if}
</header>

<section class="space-y-6">
  {#if data.skill.when_to_use}
    <div>
      <h2 class="mb-2 text-sm font-semibold tracking-wide text-[var(--sp-muted-fg)] uppercase">
        When to use
      </h2>
      <p class="text-[var(--sp-fg)]">{data.skill.when_to_use}</p>
    </div>
  {/if}

  <div>
    <h2
      class="mb-2 flex items-center gap-2 text-sm font-semibold tracking-wide text-[var(--sp-muted-fg)] uppercase"
    >
      <FileCode size="14" /> SKILL.md
    </h2>
    {#if data.body}
      <MonacoViewer value={data.body} language="markdown" readOnly height="32rem" />
    {:else}
      <p class="text-sm text-[var(--sp-muted-fg)]">
        (couldn't fetch the SKILL.md body; the download link below still works)
      </p>
    {/if}
  </div>

  <div>
    <h2 class="mb-2 text-sm font-semibold tracking-wide text-[var(--sp-muted-fg)] uppercase">
      Install
    </h2>
    <pre
      class="overflow-x-auto rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4 text-xs"><code
        >skill-pool add {data.skill.slug}</code
      ></pre>
  </div>

  <div>
    <a
      href={`${data.skill.slug}/bundle.tar.gz`}
      data-sveltekit-reload
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-2 text-sm hover:border-[var(--sp-primary)]"
    >
      <Download size="14" />
      Download bundle.tar.gz
    </a>
  </div>
</section>
