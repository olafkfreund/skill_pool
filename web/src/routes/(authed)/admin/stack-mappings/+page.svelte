<script lang="ts">
  import { AlertTriangle, CheckCircle2, Layers, Plus, Trash2 } from '@lucide/svelte';
  import type { StackMapping } from '$lib/server/api';

  let { data, form } = $props();

  // Group mappings by stack tag so the table reads "rust → [a, b, c]"
  // rather than a flat list with the tag duplicated on every row.
  const grouped = $derived.by(() => {
    const map: Record<string, StackMapping[]> = {};
    for (const m of data.mappings) {
      (map[m.stack] ??= []).push(m);
    }
    return Object.entries(map).sort(([a], [b]) => a.localeCompare(b));
  });
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Layers size="22" /> Stack mappings
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Curated <code class="rounded bg-[var(--sp-muted)] px-1">stack-tag → skill-slug</code>
    pairs that drive <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool bootstrap</code>.
    When a developer enters a project that fingerprints as
    <code class="rounded bg-[var(--sp-muted)] px-1">rust + axum + postgres</code>, the union of the
    skills mapped to any of those tags is what they're recommended to install.
  </p>
  <p class="mt-2 text-xs text-[var(--sp-muted-fg)]">
    Forward references are allowed — a mapping can name a skill that doesn't exist yet.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.added}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Added
    <code class="rounded bg-emerald-100 px-1">{form.mapping.stack} → {form.mapping.skill}</code>.
  </div>
{:else if form?.removed}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Removed
    <code class="rounded bg-emerald-100 px-1">{form.mapping.stack} → {form.mapping.skill}</code>.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

<form method="POST" action="?/add" class="mb-8 flex flex-wrap items-end gap-3">
  <label class="block">
    <span class="block text-xs text-[var(--sp-muted-fg)]">Stack tag</span>
    <input
      type="text"
      name="stack"
      required
      placeholder="rust"
      class="mt-0.5 w-40 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
    />
  </label>
  <label class="block">
    <span class="block text-xs text-[var(--sp-muted-fg)]">Skill slug</span>
    <input
      type="text"
      name="skill"
      required
      placeholder="axum-handler"
      class="mt-0.5 w-60 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
    />
  </label>
  <button
    type="submit"
    class="inline-flex h-7 items-center gap-1 rounded-[var(--sp-radius)] px-3 text-xs font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    <Plus size="12" /> Add mapping
  </button>
</form>

{#if grouped.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    No stack mappings yet. Once you add some,
    <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool bootstrap</code> will start recommending
    skills based on detected stacks.
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
          <th class="px-4 py-3">Stack tag</th>
          <th class="px-4 py-3">Skill</th>
          <th class="px-4 py-3 text-right">Action</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-[var(--sp-border)]">
        {#each grouped as [stack, rows] (stack)}
          {#each rows as m, i (m.skill)}
            <tr>
              {#if i === 0}
                <td
                  class="px-4 py-3 align-top font-mono text-xs font-semibold text-[var(--sp-fg)]"
                  rowspan={rows.length}
                >
                  {stack}
                </td>
              {/if}
              <td class="px-4 py-3 font-mono text-xs">
                <a
                  href={`/skills/${encodeURIComponent(m.skill)}`}
                  class="text-[var(--sp-fg)] hover:underline">{m.skill}</a
                >
              </td>
              <td class="px-4 py-3 text-right">
                <form method="POST" action="?/remove" class="inline-block">
                  <input type="hidden" name="stack" value={m.stack} />
                  <input type="hidden" name="skill" value={m.skill} />
                  <button
                    type="submit"
                    title={`Remove ${m.stack} → ${m.skill}`}
                    class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                  >
                    <Trash2 size="12" /> Remove
                  </button>
                </form>
              </td>
            </tr>
          {/each}
        {/each}
      </tbody>
    </table>
  </div>
{/if}
