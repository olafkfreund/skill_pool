<script lang="ts">
  import { ArrowLeft, BookOpen, Lightbulb } from '@lucide/svelte';

  let { data } = $props();
</script>

<a
  href="/help"
  class="mb-6 inline-flex items-center gap-1.5 text-xs text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
>
  <ArrowLeft size="12" /> All docs
</a>

<header class="mb-6">
  <div
    class="flex items-center gap-2 text-[10px] tracking-wider text-[var(--sp-muted-fg)] uppercase"
  >
    {#if data.doc.category === 'example'}
      <Lightbulb size="11" /> Example
    {:else}
      <BookOpen size="11" /> Reference
    {/if}
  </div>
</header>

<article class="sp-prose max-w-3xl">
  {@html data.doc.html}
</article>

<style>
  /* Hand-rolled prose styling. Tailwind v4 typography plugin would
     work too, but the project uses CSS variables for theming and this
     keeps the dependency surface flat. */
  .sp-prose :global(h1) {
    font-size: 1.875rem;
    line-height: 2.25rem;
    font-weight: 700;
    margin-bottom: 1rem;
    color: var(--sp-fg);
  }
  .sp-prose :global(h2) {
    font-size: 1.375rem;
    font-weight: 600;
    margin: 1.75rem 0 0.75rem;
    color: var(--sp-fg);
  }
  .sp-prose :global(h3) {
    font-size: 1.125rem;
    font-weight: 600;
    margin: 1.25rem 0 0.5rem;
    color: var(--sp-fg);
  }
  .sp-prose :global(h4) {
    font-size: 1rem;
    font-weight: 600;
    margin: 1rem 0 0.5rem;
    color: var(--sp-fg);
  }
  .sp-prose :global(p) {
    margin: 0.75rem 0;
    line-height: 1.7;
    color: var(--sp-fg);
  }
  .sp-prose :global(a) {
    color: var(--sp-primary);
    text-decoration: underline;
    text-underline-offset: 2px;
  }
  .sp-prose :global(code) {
    background: var(--sp-muted);
    padding: 0.1em 0.35em;
    border-radius: 0.25rem;
    font-size: 0.875em;
  }
  .sp-prose :global(pre) {
    background: var(--sp-muted);
    border: 1px solid var(--sp-border);
    padding: 1rem;
    border-radius: var(--sp-radius);
    overflow-x: auto;
    margin: 1rem 0;
  }
  .sp-prose :global(pre code) {
    background: transparent;
    padding: 0;
    font-size: 0.875rem;
  }
  .sp-prose :global(ul),
  .sp-prose :global(ol) {
    padding-left: 1.5rem;
    margin: 0.75rem 0;
  }
  .sp-prose :global(li) {
    margin: 0.25rem 0;
    line-height: 1.65;
  }
  .sp-prose :global(blockquote) {
    border-left: 3px solid var(--sp-primary);
    padding-left: 0.75rem;
    margin: 1rem 0;
    color: var(--sp-muted-fg);
    font-style: italic;
  }
  .sp-prose :global(table) {
    width: 100%;
    border-collapse: collapse;
    margin: 1rem 0;
    font-size: 0.875rem;
  }
  .sp-prose :global(th),
  .sp-prose :global(td) {
    border: 1px solid var(--sp-border);
    padding: 0.5rem 0.75rem;
    text-align: left;
  }
  .sp-prose :global(th) {
    background: var(--sp-muted);
    font-weight: 600;
  }
  .sp-prose :global(hr) {
    border: 0;
    border-top: 1px solid var(--sp-border);
    margin: 2rem 0;
  }
</style>
