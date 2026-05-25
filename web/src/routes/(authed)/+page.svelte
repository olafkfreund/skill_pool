<script lang="ts">
  import { AlertTriangle, Bot, Library, Plus, Search, Sparkles, Terminal } from '@lucide/svelte';
  import type { CatalogKind } from '$lib/server/api';

  let { data } = $props();

  // Tabs. Keep order stable: skills first (most common), then the two
  // newer kinds.
  type TabSpec = {
    kind: CatalogKind;
    label: string;
    plural: string;
    icon: typeof Library;
    blurb: string;
    newCta: string;
    emptyHint: string;
  };
  const TABS: TabSpec[] = [
    {
      kind: 'skill',
      label: 'Skills',
      plural: 'skills',
      icon: Library,
      blurb: 'Reusable patterns Claude invokes by description match.',
      newCta: 'New skill',
      emptyHint: 'Run `skill-pool publish` or use the New skill button.',
    },
    {
      kind: 'agent',
      label: 'Agents',
      plural: 'agents',
      icon: Bot,
      blurb: 'Claude Code subagents — named personas with their own system prompts.',
      newCta: 'New agent',
      emptyHint: 'Publish an agent SKILL.md with metadata.kind = "agent".',
    },
    {
      kind: 'command',
      label: 'Commands',
      plural: 'commands',
      icon: Terminal,
      blurb: 'Slash-commands that codify a repeatable workflow.',
      newCta: 'New command',
      emptyHint: 'Publish a command SKILL.md with metadata.kind = "command".',
    },
  ];
  const active = $derived(TABS.find((t) => t.kind === data.kind) ?? TABS[0]);

  function tabHref(kind: CatalogKind, query: string, semantic: boolean): string {
    const params = new URLSearchParams();
    if (kind !== 'skill') params.set('kind', kind);
    if (query) params.set('q', query);
    if (semantic) params.set('semantic', '1');
    return params.size ? `/?${params}` : '/';
  }

  function detailHref(slug: string, kind: CatalogKind): string {
    const base = `/skills/${encodeURIComponent(slug)}`;
    return kind === 'skill' ? base : `${base}?kind=${kind}`;
  }

  function newHref(kind: CatalogKind): string {
    return kind === 'skill' ? '/skills/new' : `/skills/new?kind=${kind}`;
  }

  function pct(v: number | null | undefined): string {
    if (v == null) return '';
    return `${(v * 100).toFixed(0)}%`;
  }
</script>

<header class="mb-6 flex flex-wrap items-start justify-between gap-4">
  <div>
    <h1 class="text-2xl font-semibold">Catalog</h1>
    <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
      {active.blurb} Published to
      <code class="rounded bg-[var(--sp-muted)] px-1">{data.tenant.slug}</code>.
    </p>
  </div>
  <a
    href={newHref(active.kind)}
    class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    <Plus size="14" />
    {active.newCta}
  </a>
</header>

<!-- Kind tabs -->
<nav class="mb-6 flex flex-wrap gap-2 border-b border-[var(--sp-border)] pb-2 text-sm">
  {#each TABS as tab (tab.kind)}
    {@const Icon = tab.icon}
    {@const isActive = tab.kind === active.kind}
    <a
      href={tabHref(tab.kind, data.query, data.semantic)}
      class="inline-flex items-center gap-1.5 rounded-t-[var(--sp-radius)] px-3 py-1.5 transition-colors {isActive
        ? 'border-b-2 border-[var(--sp-primary)] font-semibold text-[var(--sp-fg)]'
        : 'text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]'}"
    >
      <Icon size="14" />
      {tab.label}
    </a>
  {/each}
</nav>

<form class="mb-6 max-w-md space-y-2" data-sveltekit-reload>
  {#if data.kind !== 'skill'}
    <input type="hidden" name="kind" value={data.kind} />
  {/if}
  <div class="flex items-center gap-2">
    <div class="relative flex-1">
      <Search
        size="16"
        class="absolute top-1/2 left-3 -translate-y-1/2 text-[var(--sp-muted-fg)]"
      />
      <input
        type="search"
        name="q"
        value={data.query}
        placeholder={data.semantic
          ? `describe what the ${active.plural.slice(0, -1)} should do…`
          : `search ${active.plural}…`}
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
    title="Rank by semantic similarity. Skills only — agents and commands aren't indexed yet."
  >
    <input
      type="checkbox"
      name="semantic"
      value="1"
      checked={data.semantic}
      disabled={data.kind !== 'skill'}
      class="h-3.5 w-3.5 rounded border-[var(--sp-border)] text-[var(--sp-primary)] focus:ring-[var(--sp-primary)] disabled:opacity-60"
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
      No {active.plural} semantically match &ldquo;{data.query}&rdquo;.
    {:else if data.query}
      No {active.plural} match &ldquo;{data.query}&rdquo;.
    {:else}
      No {active.plural} published yet. {active.emptyHint}
    {/if}
  </div>
{:else}
  <ul class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
    {#each data.skills as skill (skill.slug)}
      <li>
        <a
          href={detailHref(skill.slug, data.kind)}
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
