<script lang="ts">
  import { AlertTriangle, Archive, CheckCircle2, Skull } from '@lucide/svelte';

  let { data, form } = $props();

  function fmtDate(iso: string | null): string {
    if (!iso) return '—';
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

  function daysAgo(iso: string | null): string {
    if (!iso) return '—';
    try {
      const ms = Date.now() - new Date(iso).getTime();
      const d = Math.floor(ms / (1000 * 60 * 60 * 24));
      if (d < 1) return 'today';
      if (d === 1) return '1 day ago';
      return `${d} days ago`;
    } catch {
      return '—';
    }
  }
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Skull size="22" /> Graveyard
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Skills that haven't been downloaded in a while and have few invocations. The defaults
    follow the master plan: <strong>{data.days} days</strong> stale,
    <strong>&lt;{data.maxUses} uses</strong>. Archive a skill to remove it from the catalog
    (the row stays in the database for audit). Use the URL params
    <code class="rounded bg-[var(--sp-muted)] px-1">?days=</code> and
    <code class="rounded bg-[var(--sp-muted)] px-1">?max_uses=</code> to retune.
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
    <CheckCircle2 size="16" />
    Archived <code class="rounded bg-emerald-100 px-1">{form.slug}@{form.version}</code>.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

<form class="mb-6 flex flex-wrap items-end gap-3 text-sm" data-sveltekit-reload>
  <label class="block">
    <span class="block text-xs text-[var(--sp-muted-fg)]">Stale for ≥ N days</span>
    <input
      type="number"
      name="days"
      min="1"
      max="1825"
      value={data.days}
      class="mt-0.5 w-24 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
    />
  </label>
  <label class="block">
    <span class="block text-xs text-[var(--sp-muted-fg)]">use_count &lt; N</span>
    <input
      type="number"
      name="max_uses"
      min="0"
      max="100"
      value={data.maxUses}
      class="mt-0.5 w-20 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
    />
  </label>
  <button
    type="submit"
    class="rounded-[var(--sp-radius)] px-4 py-1.5 text-sm font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    Recalculate
  </button>
</form>

{#if data.candidates.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    No decay candidates at this threshold. The catalog is healthy.
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
          <th class="px-4 py-3">Skill</th>
          <th class="px-4 py-3">Uses</th>
          <th class="px-4 py-3">Last used</th>
          <th class="px-4 py-3">Published</th>
          <th class="px-4 py-3 text-right">Action</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-[var(--sp-border)]">
        {#each data.candidates as c (c.slug)}
          <tr>
            <td class="px-4 py-3">
              <a
                href={`/skills/${encodeURIComponent(c.slug)}`}
                class="block font-medium text-[var(--sp-fg)] hover:underline"
              >
                {c.slug}
                <span class="ml-1 text-xs text-[var(--sp-muted-fg)]">v{c.version}</span>
              </a>
              <p class="mt-0.5 line-clamp-1 text-xs text-[var(--sp-muted-fg)]">{c.description}</p>
            </td>
            <td class="px-4 py-3 font-mono text-xs text-[var(--sp-fg)]">{c.use_count}</td>
            <td class="px-4 py-3 text-xs text-[var(--sp-muted-fg)]">
              {daysAgo(c.last_used_at)}
              <div class="text-[10px]">{fmtDate(c.last_used_at)}</div>
            </td>
            <td class="px-4 py-3 text-xs text-[var(--sp-muted-fg)]">{fmtDate(c.created_at)}</td>
            <td class="px-4 py-3 text-right">
              <form method="POST" action="?/archive" class="inline-block">
                <input type="hidden" name="slug" value={c.slug} />
                <button
                  type="submit"
                  title={`Archive ${c.slug}`}
                  class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-3 py-1 text-xs text-red-700 hover:bg-red-50"
                >
                  <Archive size="12" /> Archive
                </button>
              </form>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
