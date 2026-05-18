<script lang="ts">
  import { AlertTriangle, ArrowLeft, FileText, Save } from '@lucide/svelte';

  let { data, form } = $props();

  const d = $derived(data.draft);
  const tagsString = $derived(d.tags.join(', '));
  const isPending = $derived(d.status === 'pending');
</script>

<a
  href="/drafts"
  class="mb-4 inline-flex items-center gap-1.5 text-xs text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
>
  <ArrowLeft size="12" /> Back to inbox
</a>

<header class="mb-6">
  <h1 class="text-2xl font-semibold">Edit draft</h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    The bundle body (the SKILL.md content under the frontmatter) stays read-only — if you need to
    rewrite the body, discard and re-capture. Frontmatter metadata below is fully editable.
  </p>
</header>

{#if !isPending}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>
      This draft is <strong>{d.status}</strong>. Already-reviewed drafts can't be edited.
    </span>
  </div>
{/if}

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{/if}

<form method="POST" action="?/save" class="max-w-2xl space-y-4">
  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Slug</span>
    <input
      type="text"
      name="slug"
      value={d.slug}
      required
      disabled={!isPending}
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none disabled:opacity-60"
    />
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Description</span>
    <textarea
      name="description"
      rows="3"
      required
      disabled={!isPending}
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none disabled:opacity-60"
      >{d.description}</textarea
    >
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      What the skill does — third-person present tense, 1-2 sentences. Drives semantic search.
    </span>
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">When to use</span>
    <input
      type="text"
      name="when_to_use"
      value={d.when_to_use ?? ''}
      disabled={!isPending}
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none disabled:opacity-60"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Optional invocation hint. Leave empty to clear.
    </span>
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Tags</span>
    <input
      type="text"
      name="tags"
      value={tagsString}
      disabled={!isPending}
      placeholder="rust, axum, tenant"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none disabled:opacity-60"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Comma-separated. Lowercased, hyphenated.
    </span>
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Reviewer notes</span>
    <textarea
      name="notes"
      rows="3"
      disabled={!isPending}
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none disabled:opacity-60"
      >{d.notes ?? ''}</textarea
    >
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Free-form context for the reviewer. Leave empty to clear.
    </span>
  </label>

  {#if isPending}
    <button
      type="submit"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      <Save size="14" /> Save changes
    </button>
  {/if}
</form>

{#if data.skillMd}
  <section class="mt-10 max-w-2xl">
    <h2 class="mb-2 flex items-center gap-2 text-sm font-semibold">
      <FileText size="14" /> Bundle body (read-only)
    </h2>
    <pre
      class="overflow-x-auto rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4 text-xs leading-relaxed"><code
        >{data.skillMd}</code
      ></pre>
  </section>
{/if}
