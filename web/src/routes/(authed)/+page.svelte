<script lang="ts">
  import { Plus, Search } from '@lucide/svelte';

  let { data } = $props();
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

<form class="mb-6 flex max-w-md items-center gap-2" data-sveltekit-reload>
  <div class="relative flex-1">
    <Search size="16" class="absolute top-1/2 left-3 -translate-y-1/2 text-[var(--sp-muted-fg)]" />
    <input
      type="search"
      name="q"
      value={data.query}
      placeholder="search slug or description…"
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
</form>

{#if data.skills.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    {data.query
      ? `No skills match "${data.query}".`
      : 'No skills published yet. Run `skill-pool publish` from a project.'}
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
            <span class="shrink-0 text-xs text-[var(--sp-muted-fg)]">v{skill.version}</span>
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
