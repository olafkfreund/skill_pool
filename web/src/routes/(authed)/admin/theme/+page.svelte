<script lang="ts">
  import { untrack } from 'svelte';
  import { CheckCircle2, AlertTriangle, RotateCcw, Save, Tag, Download } from '@lucide/svelte';
  import { contrastRatio, wcagBadge, checkThemeContrast } from '$lib/contrast';
  import type { Theme } from '$lib/theme';

  let { data, form } = $props();

  // Editable copy. Intentionally non-reactive — `untrack` tells the compiler
  // we mean it. The `$effect` below re-syncs after a successful save.
  let theme = $state<Theme>(untrack(() => ({ ...(form?.draft ?? form?.theme ?? data.theme) })));

  $effect(() => {
    if (form?.theme && form?.saved) {
      theme = { ...form.theme };
    }
  });

  // --- Contrast badges (live, derived from current palette) ---
  const bodyBadge    = $derived(wcagBadge(contrastRatio(theme.fg, theme.bg)));
  const primaryBadge = $derived(wcagBadge(contrastRatio(theme.primaryFg, theme.primary)));
  const mutedBadge   = $derived(wcagBadge(contrastRatio(theme.mutedFg, theme.muted)));
  const mutedBgBadge = $derived(wcagBadge(contrastRatio(theme.mutedFg, theme.bg)));

  // Live failures (client-side mirror of server validation)
  const liveFailures = $derived(checkThemeContrast(theme));
  const blocked = $derived(liveFailures.length > 0);

  function confirmReset(e: MouseEvent) {
    if (!confirm('Reset all theme fields to built-in defaults? This cannot be undone.')) {
      e.preventDefault();
    }
  }

  // Only the required-string colour fields; optional/boolean fields handled separately.
  type RequiredStringKey = 'primary' | 'primaryFg' | 'accent' | 'bg' | 'fg' | 'muted' | 'mutedFg' | 'border' | 'radius' | 'brandName';
  const colorFields: Array<[string, RequiredStringKey]> = [
    ['Primary', 'primary'],
    ['Primary fg', 'primaryFg'],
    ['Accent', 'accent'],
    ['Background', 'bg'],
    ['Foreground', 'fg'],
    ['Muted', 'muted'],
    ['Muted fg', 'mutedFg'],
    ['Border', 'border'],
  ];

  function badgeClass(level: 'AAA' | 'AA' | 'AA-large' | 'fail'): string {
    return level === 'AAA'
      ? 'bg-emerald-100 text-emerald-800'
      : level === 'AA'
        ? 'bg-sky-100 text-sky-800'
        : level === 'AA-large'
          ? 'bg-amber-100 text-amber-800'
          : 'bg-red-100 text-red-800';
  }
</script>

<header class="mb-6">
  <h1 class="text-2xl font-semibold">Theme</h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Brand colours and logo. Saved theme renders for everyone on this tenant — including the login
    page. All four WCAG AA contrast pairs are checked on save.
  </p>
</header>

{#if form?.contrastFailures?.length}
  <div
    class="mb-4 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
    role="alert"
    aria-live="polite"
  >
    <div class="mb-1 flex items-center gap-2 font-medium">
      <AlertTriangle size="16" aria-hidden="true" />
      <span>Save blocked — WCAG AA contrast failures</span>
    </div>
    <ul class="ml-6 list-disc space-y-0.5">
      {#each form.contrastFailures as f}
        <li>{f.pair}: <strong>{f.ratio}</strong> (need {f.required})</li>
      {/each}
    </ul>
  </div>
{:else if form?.error}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
    role="alert"
  >
    <AlertTriangle size="16" aria-hidden="true" />
    <span>{form.error}</span>
  </div>
{:else if form?.saved}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
    role="status"
  >
    <CheckCircle2 size="16" aria-hidden="true" />
    <span>Theme saved.</span>
  </div>
{/if}

<div class="grid gap-8 lg:grid-cols-[1fr_1fr]">
  <!-- Form -->
  <form method="POST" action="?/save" class="space-y-3">
    <label class="flex items-center justify-between gap-3">
      <span class="text-sm text-[var(--sp-fg)]">Brand name</span>
      <input
        type="text"
        name="brandName"
        bind:value={theme.brandName}
        required
        class="w-48 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 text-sm"
      />
    </label>

    {#each colorFields as [label, key] (key)}
      <label class="flex items-center justify-between gap-3">
        <span class="text-sm text-[var(--sp-fg)]">{label}</span>
        <span class="flex items-center gap-2">
          <input
            type="color"
            value={theme[key]}
            oninput={(e) => (theme[key] = (e.currentTarget as HTMLInputElement).value)}
            class="h-9 w-12 cursor-pointer rounded border border-[var(--sp-border)] bg-[var(--sp-bg)]"
            aria-label="{label} colour picker"
          />
          <input
            type="text"
            name={key}
            bind:value={theme[key]}
            class="w-28 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
          />
        </span>
      </label>
    {/each}

    <label class="flex items-center justify-between gap-3">
      <span class="text-sm text-[var(--sp-fg)]">Radius</span>
      <input
        type="text"
        name="radius"
        bind:value={theme.radius}
        class="w-48 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 text-sm"
      />
    </label>

    <label class="flex items-center justify-between gap-3">
      <span class="text-sm text-[var(--sp-fg)]">
        Show "Powered by skill-pool" footer
        <span class="block text-xs text-[var(--sp-muted-fg)]">Default on (Free tier). Uncheck to hide the footer credit.</span>
      </span>
      <input
        type="checkbox"
        name="footerBranding"
        bind:checked={theme.footerBranding}
        class="h-4 w-4 cursor-pointer rounded border border-[var(--sp-border)]"
      />
    </label>

    <!-- Contrast ratio dashboard -->
    <div class="space-y-2 pt-2" aria-label="Contrast ratio checks">
      <div class="flex items-center justify-between text-xs">
        <span class="text-[var(--sp-muted-fg)]">Body text (fg / bg)</span>
        <span class="rounded px-2 py-0.5 font-medium {badgeClass(bodyBadge.level)}">
          {bodyBadge.label}
        </span>
      </div>
      <div class="flex items-center justify-between text-xs">
        <span class="text-[var(--sp-muted-fg)]">Primary button (primary-fg / primary)</span>
        <span class="rounded px-2 py-0.5 font-medium {badgeClass(primaryBadge.level)}">
          {primaryBadge.label}
        </span>
      </div>
      <div class="flex items-center justify-between text-xs">
        <span class="text-[var(--sp-muted-fg)]">Secondary text (muted-fg / muted)</span>
        <span class="rounded px-2 py-0.5 font-medium {badgeClass(mutedBadge.level)}">
          {mutedBadge.label}
        </span>
      </div>
      <div class="flex items-center justify-between text-xs">
        <span class="text-[var(--sp-muted-fg)]">Secondary text (muted-fg / bg)</span>
        <span class="rounded px-2 py-0.5 font-medium {badgeClass(mutedBgBadge.level)}">
          {mutedBgBadge.label}
        </span>
      </div>
    </div>

    <!-- Live client-side failure list -->
    {#if liveFailures.length > 0}
      <div
        class="rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-2.5 text-xs text-amber-900"
        role="status"
        aria-live="polite"
      >
        <p class="mb-1 font-medium">Fix contrast before saving:</p>
        <ul class="ml-4 list-disc space-y-0.5">
          {#each liveFailures as f}
            <li>{f.pair}: {f.ratio} (need {f.required})</li>
          {/each}
        </ul>
      </div>
    {/if}

    <div class="flex items-center gap-3 pt-2">
      <button
        type="submit"
        disabled={blocked}
        aria-disabled={blocked}
        class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-50"
        style="background: var(--sp-primary); color: var(--sp-primary-fg);"
      >
        <Save size="14" aria-hidden="true" /> Save
      </button>
      <button
        type="submit"
        formaction="?/reset"
        onclick={confirmReset}
        class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm"
      >
        <RotateCcw size="14" aria-hidden="true" /> Reset to defaults
      </button>
    </div>
  </form>

  <!-- Live catalog card preview — updates reactively with every colour change -->
  <section aria-label="Live catalog card preview">
    <p class="mb-3 text-xs font-medium tracking-widest uppercase text-[var(--sp-muted-fg)]">
      Live preview
    </p>

    <!-- Catalog card -->
    <article
      class="overflow-hidden shadow-sm"
      style="
        background: {theme.bg};
        color: {theme.fg};
        border: 1px solid {theme.border};
        border-radius: {theme.radius};
      "
    >
      <!-- Card header -->
      <div
        class="flex items-start justify-between gap-3 px-4 pt-4 pb-3"
        style="border-bottom: 1px solid {theme.border};"
      >
        <div class="min-w-0">
          <h2 class="truncate text-sm font-semibold leading-snug" style="color: {theme.fg};">
            react-query-state-sync
          </h2>
          <p class="mt-0.5 text-xs" style="color: {theme.mutedFg};">v2.4.1 · skill</p>
        </div>
        <span
          class="shrink-0 rounded-full px-2 py-0.5 text-xs font-medium"
          style="background: {theme.accent}; color: {theme.primaryFg};"
        >
          new
        </span>
      </div>

      <!-- Card body -->
      <div class="px-4 py-3">
        <p class="text-xs leading-relaxed" style="color: {theme.mutedFg};">
          Bidirectional sync for TanStack Query caches across browser tabs using BroadcastChannel.
          Zero config, tree-shakeable, &lt;2 kB gzipped.
        </p>

        <!-- Tags -->
        <div class="mt-3 flex flex-wrap gap-1.5">
          {#each ['react', 'state', 'sync'] as tag}
            <span
              class="inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-xs"
              style="background: {theme.muted}; color: {theme.mutedFg};"
            >
              <Tag size="10" aria-hidden="true" />
              {tag}
            </span>
          {/each}
        </div>
      </div>

      <!-- Card footer -->
      <div
        class="flex items-center justify-between gap-3 px-4 py-3"
        style="background: {theme.muted}; border-top: 1px solid {theme.border};"
      >
        <span class="text-xs" style="color: {theme.mutedFg};">
          Used 142 times
        </span>
        <button
          type="button"
          class="inline-flex items-center gap-1.5 rounded px-3 py-1 text-xs font-medium"
          style="background: {theme.primary}; color: {theme.primaryFg}; border-radius: {theme.radius};"
          tabindex="-1"
          aria-hidden="true"
        >
          <Download size="11" aria-hidden="true" /> Install
        </button>
      </div>
    </article>

    <!-- Brand watermark -->
    <p class="mt-3 text-right text-xs" style="color: {theme.mutedFg};">
      {theme.brandName || 'skill-pool'} catalog
    </p>
  </section>
</div>
