<script lang="ts">
  import { AlertTriangle, CheckCircle2, GitMerge, Inbox, Trash2, Send } from '@lucide/svelte';
  import type { Draft } from '$lib/server/api';

  let { data, form } = $props();

  const FILTERS = [
    { key: 'pending', label: 'Pending' },
    { key: 'published', label: 'Published' },
    { key: 'discarded', label: 'Discarded' },
    { key: 'all', label: 'All' },
  ] as const;

  function fmtDate(iso: string): string {
    try {
      return new Date(iso).toLocaleString(undefined, {
        year: 'numeric',
        month: 'short',
        day: 'numeric',
        hour: '2-digit',
        minute: '2-digit',
      });
    } catch {
      return iso;
    }
  }

  function originBadge(o: Draft['origin']): string {
    return o === 'capture-scorer'
      ? 'bg-amber-100 text-amber-800'
      : o === 'claude-hook'
        ? 'bg-purple-100 text-purple-800'
        : o === 'web'
          ? 'bg-sky-100 text-sky-800'
          : 'bg-slate-100 text-slate-700';
  }

  function statusBadge(s: Draft['status']): string {
    return s === 'pending'
      ? 'bg-amber-100 text-amber-800'
      : s === 'published'
        ? 'bg-emerald-100 text-emerald-800'
        : 'bg-slate-100 text-slate-600';
  }
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Inbox size="22" /> Drafts inbox
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Captured skills awaiting review. Publish promotes a draft into the catalog with an assigned
    version. Discard removes it from the inbox (kept for telemetry).
  </p>
</header>

<nav class="mb-6 flex gap-2 text-sm">
  {#each FILTERS as f (f.key)}
    {@const active = data.status === f.key}
    <a
      href={f.key === 'pending' ? '/drafts' : `/drafts?status=${f.key}`}
      class="rounded-[var(--sp-radius)] px-3 py-1 transition-colors {active
        ? 'bg-[var(--sp-primary)] text-[var(--sp-primary-fg)]'
        : 'border border-[var(--sp-border)] text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]'}"
    >
      {f.label}
    </a>
  {/each}
</nav>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.published}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" />
    Published <code class="rounded bg-emerald-100 px-1">{form.slug}@{form.version}</code>.
  </div>
{:else if form?.discarded}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Draft discarded.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

{#if data.drafts.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    No {data.status === 'all' ? '' : data.status} drafts. Capture from the CLI with
    <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool capture &lt;dir&gt;</code>.
  </div>
{:else}
  <ul class="space-y-4">
    {#each data.drafts as d (d.id)}
      <li
        class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
      >
        <div class="flex flex-wrap items-start justify-between gap-3">
          <div class="min-w-0 flex-1">
            <div class="flex flex-wrap items-center gap-2">
              <h2 class="text-base font-semibold text-[var(--sp-fg)]">{d.slug}</h2>
              <span class="rounded-full px-2 py-0.5 text-xs font-medium {statusBadge(d.status)}">
                {d.status}{d.published_version ? `@${d.published_version}` : ''}
              </span>
              <span class="rounded-full px-2 py-0.5 text-xs font-medium {originBadge(d.origin)}">
                {d.origin}
              </span>
            </div>
            <p class="mt-1 text-sm text-[var(--sp-fg)]">{d.description}</p>
            {#if d.merge_proposal_slug}
              <a
                href={`/skills/${encodeURIComponent(d.merge_proposal_slug)}`}
                class="mt-2 inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 px-2 py-1 text-xs text-amber-900 hover:bg-amber-100"
                title="Embedding dedup flagged this draft as a near-duplicate"
              >
                <GitMerge size="12" />
                Looks like
                <code class="font-mono">{d.merge_proposal_slug}</code>
                {#if d.merge_proposal_similarity != null}
                  <span class="text-amber-700">
                    ({(d.merge_proposal_similarity * 100).toFixed(0)}% match)
                  </span>
                {/if}
              </a>
            {/if}
            {#if d.when_to_use}
              <p class="mt-1 text-xs text-[var(--sp-muted-fg)]">
                <span class="font-medium">When:</span>
                {d.when_to_use}
              </p>
            {/if}
            {#if d.notes}
              <p
                class="mt-2 rounded border-l-2 border-[var(--sp-primary)] bg-[var(--sp-bg)] px-2 py-1 text-xs text-[var(--sp-muted-fg)]"
              >
                <span class="font-medium">Reviewer note:</span>
                {d.notes}
              </p>
            {/if}
            {#if d.tags.length > 0}
              <div class="mt-2 flex flex-wrap gap-1">
                {#each d.tags as t (t)}
                  <span
                    class="rounded bg-[var(--sp-bg)] px-1.5 py-0.5 text-[10px] text-[var(--sp-muted-fg)]"
                  >
                    {t}
                  </span>
                {/each}
              </div>
            {/if}
            <div class="mt-2 text-[11px] text-[var(--sp-muted-fg)]">
              created {fmtDate(d.created_at)}
              {#if d.reviewed_at}
                · reviewed {fmtDate(d.reviewed_at)}
              {/if}
            </div>
          </div>

          {#if d.status === 'pending'}
            <div class="flex shrink-0 flex-col gap-2 sm:flex-row sm:items-end">
              <form
                method="POST"
                action="?/publish"
                class="flex flex-col gap-2 sm:flex-row sm:items-end"
              >
                <input type="hidden" name="id" value={d.id} />
                <label class="text-xs">
                  <span class="block text-[var(--sp-muted-fg)]">Slug</span>
                  <input
                    type="text"
                    name="slug"
                    value={d.slug}
                    class="w-40 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
                  />
                </label>
                <label class="text-xs">
                  <span class="block text-[var(--sp-muted-fg)]">Version</span>
                  <input
                    type="text"
                    name="version"
                    placeholder="1.0.0"
                    required
                    class="w-24 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
                  />
                </label>
                <button
                  type="submit"
                  class="inline-flex h-7 items-center gap-1 self-end rounded-[var(--sp-radius)] px-3 text-xs font-medium"
                  style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                >
                  <Send size="12" /> Publish
                </button>
              </form>
              <form method="POST" action="?/discard">
                <input type="hidden" name="id" value={d.id} />
                <button
                  type="submit"
                  class="inline-flex h-7 items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-3 text-xs text-red-700 hover:bg-red-50"
                >
                  <Trash2 size="12" /> Discard
                </button>
              </form>
            </div>
          {/if}
        </div>
      </li>
    {/each}
  </ul>
{/if}
