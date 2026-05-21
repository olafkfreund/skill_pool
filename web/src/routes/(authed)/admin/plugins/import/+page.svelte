<script lang="ts">
  import { AlertTriangle, CheckCircle2, Download, ExternalLink, Package } from '@lucide/svelte';

  import { untrack } from 'svelte';

  let { form } = $props();

  // `form` is the discriminated union returned by the action; the
  // `notYetAvailable` and `tracking_issue` keys live only on the 503
  // branch. Widening to a record lets us read both branches without
  // duplicating the type plumbing.
  const formAny = $derived(form as Record<string, unknown> | null);
  const notYetAvailable = $derived(!!formAny?.notYetAvailable);
  const trackingIssue = $derived(
    typeof formAny?.tracking_issue === 'number' ? (formAny.tracking_issue as number) : 32,
  );

  // Seed the controlled inputs from the echoed form values once — after
  // mount the component owns the field state. `untrack` quiets the Svelte
  // warning about initial-value capture; this is the desired behaviour.
  let url = $state(
    untrack(() => (typeof formAny?.url === 'string' ? (formAny.url as string) : '')),
  );
  let refreshInterval = $state(
    untrack(() =>
      typeof formAny?.refresh_interval_secs === 'string'
        ? (formAny.refresh_interval_secs as string)
        : '86400',
    ),
  );
</script>

<header class="mb-6">
  <nav class="mb-2 text-xs text-[var(--sp-muted-fg)]">
    <a href="/admin/plugins" class="hover:underline">Plugins</a>
    <span class="mx-1">/</span>
    <span>Import plugin</span>
  </nav>
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Package size="22" /> Import plugin
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Mirror an upstream git repository into this tenant's marketplace. skill-pool clones it locally
    and re-serves it from the
    <code class="rounded bg-[var(--sp-muted)] px-1">/git/plugins/</code> endpoint.
  </p>
</header>

<!-- Not-yet-available banner — always visible until #32 ships. -->
<div
  class="mb-6 flex items-start gap-3 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-4 text-sm text-amber-800"
  role="status"
>
  <AlertTriangle size="18" class="mt-0.5 shrink-0" />
  <div class="space-y-1">
    <p class="font-medium">Plugin import is not yet available.</p>
    <p class="text-xs">
      The async import worker lands in tracking issue
      <a
        href={`https://github.com/olafkfreund/skill_pool/issues/${trackingIssue}`}
        target="_blank"
        rel="noopener noreferrer"
        class="inline-flex items-center gap-0.5 underline"
      >
        #{trackingIssue}
        <ExternalLink size="10" />
      </a>. Once that ships you'll be able to paste a git URL here and skill-pool will mirror the
      plugin into this tenant's marketplace.
    </p>
  </div>
</div>

{#if form?.imported}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Import job enqueued
    {#if form?.job_id}
      (<code class="rounded bg-emerald-100 px-1">job {form.job_id}</code>)
    {/if}
    for
    <code class="rounded bg-emerald-100 px-1">{form?.url}</code>.
  </div>
{:else if form?.error && !notYetAvailable}
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
      Git URL <span class="text-red-500">*</span>
    </span>
    <input
      type="url"
      name="url"
      required
      bind:value={url}
      aria-disabled={notYetAvailable ? 'true' : undefined}
      placeholder="https://github.com/acme-corp/formatter.git"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Any HTTPS git URL Claude Code's
      <code class="rounded bg-[var(--sp-muted)] px-1">git clone</code> understands.
    </span>
  </label>

  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Refresh interval (seconds)</span>
    <input
      type="number"
      name="refresh_interval_secs"
      min="300"
      step="1"
      bind:value={refreshInterval}
      aria-disabled={notYetAvailable ? 'true' : undefined}
      placeholder="86400"
      class="mt-1 w-40 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Optional. Defaults to 86 400 (24 h) when the worker pulls the upstream. Min 300 s.
    </span>
  </label>

  <div class="flex items-center gap-3 pt-2">
    <button
      type="submit"
      class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-60"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      <Download size="14" /> Enqueue import
    </button>
    <a href="/admin/plugins" class="text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]">
      Cancel
    </a>
  </div>
</form>
