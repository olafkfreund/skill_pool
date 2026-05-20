<script lang="ts">
  import { AlertTriangle, FolderGit2 } from '@lucide/svelte';

  let { form } = $props();
</script>

<header class="mb-6">
  <nav class="mb-2 text-xs text-[var(--sp-muted-fg)]">
    <a href="/admin/projects" class="hover:underline">Projects</a>
    <span class="mx-1">/</span>
    <span>New project</span>
  </nav>
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <FolderGit2 size="22" /> New project
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Create a named project bundle. After creation you can curate which skills, agents, and commands
    belong to it.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-6 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{/if}

<form method="POST" class="max-w-xl space-y-5">
  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">
      Slug <span class="text-red-500">*</span>
    </span>
    <input
      type="text"
      name="slug"
      required
      value={form?.slug ?? ''}
      placeholder="acme-billing-service"
      pattern="[a-z0-9][a-z0-9\-]*[a-z0-9]|[a-z0-9]"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Lowercase letters, digits, and hyphens only. Used in
      <code class="rounded bg-[var(--sp-muted)] px-1">manifest.toml</code> and the CLI.
    </span>
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">
      Name <span class="text-red-500">*</span>
    </span>
    <input
      type="text"
      name="name"
      required
      value={form?.name ?? ''}
      placeholder="Acme Billing Service"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Description</span>
    <textarea
      name="description"
      rows="3"
      placeholder="Short description of what this project does…"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    >{form?.description ?? ''}</textarea>
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Git remote</span>
    <input
      type="url"
      name="git_remote"
      value={form?.git_remote ?? ''}
      placeholder="https://github.com/acme/billing-service"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Optional. When set, <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool bootstrap</code>
      auto-discovers this project by matching the repo's origin URL.
    </span>
  </label>

  <div class="flex items-center gap-3 pt-2">
    <button
      type="submit"
      class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Create project
    </button>
    <a
      href="/admin/projects"
      class="text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
    >
      Cancel
    </a>
  </div>
</form>
