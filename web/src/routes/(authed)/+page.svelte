<script lang="ts">
  import { AlertTriangle, Plus, Search, Sparkles } from '@lucide/svelte';

  let { data } = $props();

  function pct(v: number | null | undefined): string {
    if (v == null) return '';
    return `${(v * 100).toFixed(0)}%`;
  }
</script>

<header class="mb-8 flex flex-wrap items-start justify-between gap-4">
  <div>
    <h1 class="text-2xl font-semibold">Catalog</h1>
    <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
      Skills published to <code class="rounded bg-[var(--sp-muted)] px-1">{data.tenant.slug}</code>.
    </p>
  </div>
  <a
    href="/skills/new"
    class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    <Plus size="14" /> New skill
  </a>
</header>

<form class="mb-6 max-w-md space-y-2" data-sveltekit-reload>
  <div class="flex items-center gap-2">
    <div class="relative flex-1">
      <Search size="16" class="absolute top-1/2 left-3 -translate-y-1/2 text-[var(--sp-muted-fg)]" />
      <input
        type="search"
        name="q"
        value={data.query}
        placeholder={data.semantic
          ? 'describe what the skill should do…'
          : 'search slug or description…'}
        class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] py-2 pr-3 pl-9 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
    </div>
    <button
      type="submit"
      class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Search
    </button>
  </div>
  <label
    class="inline-flex cursor-pointer items-center gap-2 text-xs text-[var(--sp-muted-fg)]"
    title="Rank results by semantic similarity to your query (requires server to be built with --features fastembed)."
  >
    <input
      type="checkbox"
      name="semantic"
      value="1"
      checked={data.semantic}
      class="h-3.5 w-3.5 rounded border-[var(--sp-border)] text-[var(--sp-primary)] focus:ring-[var(--sp-primary)]"
    />
    <Sparkles size="12" />
    <span>Semantic search</span>
  </label>
</form>

{#if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{data.error}</span>
  </div>
{/if}

{#if data.skills.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    {#if data.query && data.semantic}
      No skills semantically match &ldquo;{data.query}&rdquo;.
    {:else if data.query}
      No skills match &ldquo;{data.query}&rdquo;.
    {:else}
      No skills published yet. Run <code>skill-pool publish</code> from a project.
    {/if}
  </div>
{:else}
  <ul class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
    {#each data.skills as skill (skill.slug)}
      <li>
        <a
          href={`/skills/${encodeURIComponent(skill.slug)}`}
          class="block h-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-5 transition-colors hover:border-[var(--sp-primary)]"
        >
          <header class="mb-2 flex items-baseline justify-between gap-2">
            <h2 class="truncate font-semibold text-[var(--sp-fg)]">{skill.slug}</h2>
            <div class="flex shrink-0 items-center gap-2">
              {#if skill.similarity != null}
                <span
                  class="inline-flex items-center gap-1 rounded-full bg-[var(--sp-primary)] px-2 py-0.5 text-[10px] font-medium"
                  style="color: var(--sp-primary-fg);"
                  title="Cosine similarity to your query"
                >
                  <Sparkles size="10" />
                  {pct(skill.similarity)}
                </span>
              {/if}
              <span class="text-xs text-[var(--sp-muted-fg)]">v{skill.version}</span>
            </div>
          </header>
          <p class="line-clamp-3 text-sm text-[var(--sp-muted-fg)]">{skill.description}</p>
          {#if skill.tags.length > 0}
            <div class="mt-3 flex flex-wrap gap-1">
              {#each skill.tags as tag (tag)}
                <span
                  class="rounded-full px-2 py-0.5 text-xs"
                  style="background: var(--sp-bg); color: var(--sp-fg); border: 1px solid var(--sp-border);"
                  >{tag}</span
                >
              {/each}
            </div>
          {/if}
        </a>
      </li>
    {/each}
  </ul>
{/if}
