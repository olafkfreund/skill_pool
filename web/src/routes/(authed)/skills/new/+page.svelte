<script lang="ts">
  import { untrack } from 'svelte';
  import { ArrowLeft, CheckCircle2, AlertTriangle, Send, Wand2 } from '@lucide/svelte';
  import MonacoViewer from '$lib/MonacoViewer.svelte';

  let { data, form } = $props();

  // Editable state. Seeded once from the load template OR from the draft a
  // failed action sent back. The `untrack` keeps the compiler quiet about
  // reactive-init intent — we'll re-sync on validation results below.
  let slug = $state(untrack(() => form?.draft?.slug ?? ''));
  let version = $state(untrack(() => form?.draft?.version ?? ''));
  let tags = $state(untrack(() => form?.draft?.tags ?? ''));
  let skillMd = $state(untrack(() => form?.draft?.skillMd ?? data.template));

  // When the validate action returns auto-filled metadata, populate the
  // frontmatter fields the user hasn't touched yet.
  $effect(() => {
    if (form?.validated) {
      if (!slug && form.validated.name) slug = form.validated.name;
      if (!tags && form.validated.tags?.length) tags = form.validated.tags.join(', ');
    }
  });
</script>

<a
  href="/"
  class="mb-6 inline-flex items-center gap-1 text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
>
  <ArrowLeft size="14" /> Catalog
</a>

<header class="mb-6">
  <h1 class="text-2xl font-semibold">New skill</h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Write a SKILL.md, validate it (frontmatter / secret scan / WCAG-equivalent quality gates), then
    publish. Publishing creates the first version; subsequent versions reuse the same slug.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.validated}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" class="mt-0.5 shrink-0" />
    <div>
      <strong>Validates clean.</strong>
      {#if form.validated.name}<span> name=<code>{form.validated.name}</code>;</span>{/if}
      {#if form.validated.description}<span> description="{form.validated.description}";</span>{/if}
      {#if form.validated.tags?.length}<span> tags={form.validated.tags.join(', ')}.</span>{/if}
    </div>
  </div>
{/if}

<form method="POST" class="space-y-4">
  <div class="grid gap-3 sm:grid-cols-[1fr_140px_2fr]">
    <label class="block text-sm">
      <span class="text-[var(--sp-muted-fg)]">Slug</span>
      <input
        type="text"
        name="slug"
        bind:value={slug}
        placeholder="my-new-skill"
        required
        pattern="[a-z0-9][a-z0-9-]*"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
    </label>
    <label class="block text-sm">
      <span class="text-[var(--sp-muted-fg)]">Version</span>
      <input
        type="text"
        name="version"
        bind:value={version}
        placeholder="1.0.0"
        required
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
    </label>
    <label class="block text-sm">
      <span class="text-[var(--sp-muted-fg)]">Tags (comma-separated)</span>
      <input
        type="text"
        name="tags"
        bind:value={tags}
        placeholder="ops, ci, deploy"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
    </label>
  </div>

  <div>
    <label for="skill-md-editor" class="mb-1 block text-sm text-[var(--sp-muted-fg)]">
      SKILL.md
    </label>
    <MonacoViewer
      value={skillMd}
      language="markdown"
      height="32rem"
      onChange={(next) => (skillMd = next)}
    />
    <!-- The form serialises this hidden input rather than reading from Monaco directly,
         so the action receives whatever Monaco's onChange last produced. -->
    <input type="hidden" name="skillMd" value={skillMd} />
  </div>

  <div class="flex flex-wrap items-center gap-3">
    <button
      type="submit"
      formaction="?/validate"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm font-medium hover:border-[var(--sp-primary)]"
    >
      <Wand2 size="14" /> Validate
    </button>
    <button
      type="submit"
      formaction="?/publish"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      <Send size="14" /> Publish
    </button>
    <p class="text-xs text-[var(--sp-muted-fg)]">
      Validate runs the same lints as publish, without writing anything to the registry.
    </p>
  </div>
</form>
