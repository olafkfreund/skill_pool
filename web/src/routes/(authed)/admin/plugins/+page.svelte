<script lang="ts">
  import { AlertTriangle, CheckCircle2, Download, Package, Plus, Trash2 } from '@lucide/svelte';

  let { data, form } = $props();

  const isCurator = $derived(data.userRole === 'curator' || data.userRole === 'admin');

  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString(undefined, {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
      });
    } catch {
      return iso;
    }
  }

  const MODES: Array<{ value: 'internal' | 'mirror' | 'external'; label: string }> = [
    { value: 'internal', label: 'Internal' },
    { value: 'mirror', label: 'Mirror' },
    { value: 'external', label: 'External' },
  ];

  function chipHref(value: 'internal' | 'mirror' | 'external' | null): string {
    if (value === null) return '/admin/plugins';
    const params = new URLSearchParams({ sourcing_mode: value });
    return `/admin/plugins?${params}`;
  }
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Package size="22" /> Plugins
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Plugins bundle skills, agents, commands, hooks, MCP servers, and monitors into one installable
    unit that Claude Code consumes via
    <code class="rounded bg-[var(--sp-muted)] px-1">/plugin install</code>.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.archived}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Archived plugin
    <code class="rounded bg-emerald-100 px-1">{form.slug}@{form.version}</code>.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

<!-- Filter chips + action buttons row -->
<div class="mb-6 flex flex-wrap items-center justify-between gap-3">
  <div class="flex flex-wrap items-center gap-2">
    <span class="text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">Filter</span
    >
    <a
      href={chipHref(null)}
      class="rounded-full border px-3 py-1 text-xs"
      style={data.sourcingMode === null
        ? 'background: var(--sp-primary); color: var(--sp-primary-fg); border-color: var(--sp-primary);'
        : 'border-color: var(--sp-border); color: var(--sp-muted-fg);'}
      aria-current={data.sourcingMode === null ? 'page' : undefined}
    >
      All
    </a>
    {#each MODES as mode (mode.value)}
      <a
        href={chipHref(mode.value)}
        class="rounded-full border px-3 py-1 text-xs"
        style={data.sourcingMode === mode.value
          ? 'background: var(--sp-primary); color: var(--sp-primary-fg); border-color: var(--sp-primary);'
          : 'border-color: var(--sp-border); color: var(--sp-muted-fg);'}
        aria-current={data.sourcingMode === mode.value ? 'page' : undefined}
      >
        {mode.label}
      </a>
    {/each}
  </div>

  {#if isCurator}
    <div class="flex items-center gap-2">
      <a
        href="/admin/plugins/import"
        class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-2 text-sm font-medium hover:border-[var(--sp-primary)]"
      >
        <Download size="14" /> Import plugin
      </a>
      <a
        href="/admin/plugins/new"
        class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
        style="background: var(--sp-primary); color: var(--sp-primary-fg);"
      >
        <Plus size="14" /> New plugin
      </a>
    </div>
  {/if}
</div>

{#if data.plugins.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    {#if data.sourcingMode}
      No <code class="rounded bg-[var(--sp-muted)] px-1">{data.sourcingMode}</code> plugins yet.
    {:else}
      No plugins yet. Create one to bundle skills, agents, and commands for Claude Code.
    {/if}
  </div>
{:else}
  <div
    class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]"
  >
    <table class="w-full text-sm">
      <thead
        class="bg-[var(--sp-bg)] text-left text-xs tracking-wide text-[var(--sp-muted-fg)] uppercase"
      >
        <tr>
          <th class="px-4 py-3">Slug</th>
          <th class="px-4 py-3">Version</th>
          <th class="px-4 py-3">Sourcing</th>
          <th class="px-4 py-3">Tags</th>
          <th class="px-4 py-3">Created</th>
          <th class="px-4 py-3 text-right">Actions</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-[var(--sp-border)]">
        {#each data.plugins as p (`${p.slug}@${p.version}`)}
          <tr>
            <td class="px-4 py-3 font-mono text-xs font-semibold text-[var(--sp-fg)]">
              <a href={`/admin/plugins/${encodeURIComponent(p.slug)}`} class="hover:underline">
                {p.slug}
              </a>
              {#if p.name && p.name !== p.slug}
                <div class="mt-0.5 font-sans text-xs font-normal text-[var(--sp-muted-fg)]">
                  {p.name}
                </div>
              {/if}
            </td>
            <td class="px-4 py-3 font-mono text-xs text-[var(--sp-muted-fg)]">{p.version}</td>
            <td class="px-4 py-3">
              <span
                class="rounded-full border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-0.5 text-xs text-[var(--sp-muted-fg)]"
              >
                {p.sourcing_mode}
              </span>
            </td>
            <td class="px-4 py-3">
              {#if p.tags.length === 0}
                <span class="text-xs text-[var(--sp-muted-fg)]">—</span>
              {:else}
                <div class="flex flex-wrap gap-1">
                  {#each p.tags.slice(0, 3) as tag (tag)}
                    <span
                      class="rounded-full bg-[var(--sp-bg)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--sp-muted-fg)]"
                    >
                      {tag}
                    </span>
                  {/each}
                  {#if p.tags.length > 3}
                    <span class="text-[10px] text-[var(--sp-muted-fg)]">
                      +{p.tags.length - 3}
                    </span>
                  {/if}
                </div>
              {/if}
            </td>
            <td class="px-4 py-3 text-[var(--sp-muted-fg)]">{fmtDate(p.created_at)}</td>
            <td class="px-4 py-3 text-right">
              <div class="inline-flex items-center gap-2">
                <a
                  href={`/admin/plugins/${encodeURIComponent(p.slug)}`}
                  class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-1 text-xs hover:border-[var(--sp-primary)]"
                >
                  Edit
                </a>
                {#if isCurator}
                  <form
                    method="POST"
                    action="?/archive"
                    class="inline-block"
                    onsubmit={(e) => {
                      if (
                        !confirm(
                          `Archive plugin "${p.slug}@${p.version}"? It stops appearing in the marketplace.`,
                        )
                      ) {
                        e.preventDefault();
                      }
                    }}
                  >
                    <input type="hidden" name="slug" value={p.slug} />
                    <input type="hidden" name="version" value={p.version} />
                    <button
                      type="submit"
                      title={`Archive ${p.slug}@${p.version}`}
                      class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                    >
                      <Trash2 size="12" /> Archive
                    </button>
                  </form>
                {/if}
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
