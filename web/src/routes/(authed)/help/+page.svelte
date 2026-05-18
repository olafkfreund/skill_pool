<script lang="ts">
  import { BookOpen, FileText, Lightbulb } from '@lucide/svelte';

  let { data } = $props();
</script>

<header class="mb-8">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <BookOpen size="22" /> Help &amp; Docs
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    The same Markdown that lives in the repo's <code class="rounded bg-[var(--sp-muted)] px-1"
      >docs/</code
    > folder. Bundled at build time so it ships in lockstep with the portal — no second copy to drift.
  </p>
</header>

<section class="mb-10">
  <h2 class="mb-3 flex items-center gap-2 text-lg font-semibold">
    <Lightbulb size="18" /> Real-life examples
  </h2>
  <p class="mb-4 text-sm text-[var(--sp-muted-fg)]">
    Step-by-step walkthroughs that go from "I have a problem" to "the team has a skill." Read these
    first if you're new.
  </p>
  {#if data.examples.length === 0}
    <p class="text-sm text-[var(--sp-muted-fg)] italic">No examples yet.</p>
  {:else}
    <ul class="grid gap-4 sm:grid-cols-2">
      {#each data.examples as doc (doc.slug)}
        <li>
          <a
            href={`/help/${encodeURIComponent(doc.slug)}`}
            class="block h-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-5 transition-colors hover:border-[var(--sp-primary)]"
          >
            <h3 class="mb-1 font-semibold text-[var(--sp-fg)]">{doc.title}</h3>
            <p class="line-clamp-3 text-sm text-[var(--sp-muted-fg)]">{doc.excerpt}</p>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section>
  <h2 class="mb-3 flex items-center gap-2 text-lg font-semibold">
    <FileText size="18" /> Reference documentation
  </h2>
  <p class="mb-4 text-sm text-[var(--sp-muted-fg)]">
    Technical contracts: HTTP API surface, architecture, manifest schema, lifecycle pipelines.
  </p>
  {#if data.reference.length === 0}
    <p class="text-sm text-[var(--sp-muted-fg)] italic">No reference docs.</p>
  {:else}
    <ul class="grid gap-4 sm:grid-cols-2">
      {#each data.reference as doc (doc.slug)}
        <li>
          <a
            href={`/help/${encodeURIComponent(doc.slug)}`}
            class="block h-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-5 transition-colors hover:border-[var(--sp-primary)]"
          >
            <h3 class="mb-1 font-semibold text-[var(--sp-fg)]">{doc.title}</h3>
            <p class="line-clamp-3 text-sm text-[var(--sp-muted-fg)]">{doc.excerpt}</p>
          </a>
        </li>
      {/each}
    </ul>
  {/if}
</section>
