<script lang="ts">
  import { AlertTriangle, CheckCircle2, FolderGit2, Plus, Trash2 } from '@lucide/svelte';

  let { data, form } = $props();

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
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <FolderGit2 size="22" /> Projects
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Projects bundle a curated set of skills, agents, and commands for a specific codebase.
    Developers cloning the repo get exactly that bundle via
    <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool bootstrap</code>.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.deleted}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Deleted project
    <code class="rounded bg-emerald-100 px-1">{form.slug}</code>.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

<div class="mb-6 flex justify-end">
  <a
    href="/admin/projects/new"
    class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    <Plus size="14" /> New project
  </a>
</div>

{#if data.projects.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    No projects yet. Create one to bundle specific skills, agents, and commands for a codebase.
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
          <th class="px-4 py-3">Name</th>
          <th class="px-4 py-3">Items</th>
          <th class="px-4 py-3">Git remote</th>
          <th class="px-4 py-3">Updated</th>
          <th class="px-4 py-3 text-right">Actions</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-[var(--sp-border)]">
        {#each data.projects as p (p.slug)}
          <tr>
            <td class="px-4 py-3 font-mono text-xs font-semibold text-[var(--sp-fg)]">
              {p.slug}
            </td>
            <td class="px-4 py-3 text-[var(--sp-fg)]">{p.name}</td>
            <td class="px-4 py-3 text-[var(--sp-muted-fg)]">
              {#if (p.item_count ?? 0) === 0}
                <span class="text-xs">—</span>
              {:else}
                {p.item_count} {(p.item_count ?? 0) === 1 ? 'item' : 'items'}
              {/if}
            </td>
            <td class="px-4 py-3 font-mono text-xs text-[var(--sp-muted-fg)]">
              {p.git_remote ?? '—'}
            </td>
            <td class="px-4 py-3 text-[var(--sp-muted-fg)]">{fmtDate(p.updated_at)}</td>
            <td class="px-4 py-3 text-right">
              <div class="inline-flex items-center gap-2">
                <a
                  href={`/admin/projects/${encodeURIComponent(p.slug)}`}
                  class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-1 text-xs hover:border-[var(--sp-primary)]"
                >
                  Edit
                </a>
                <form
                  method="POST"
                  action="?/delete"
                  class="inline-block"
                  onsubmit={(e) => {
                    if (!confirm(`Delete project "${p.slug}"? This cannot be undone.`)) {
                      e.preventDefault();
                    }
                  }}
                >
                  <input type="hidden" name="slug" value={p.slug} />
                  <button
                    type="submit"
                    title={`Delete ${p.slug}`}
                    class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                  >
                    <Trash2 size="12" /> Delete
                  </button>
                </form>
              </div>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
