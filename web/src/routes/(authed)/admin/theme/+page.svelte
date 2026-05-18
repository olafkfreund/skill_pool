<script lang="ts">
  import { untrack } from 'svelte';
  import { CheckCircle2, AlertTriangle, RotateCcw, Save } from '@lucide/svelte';
  import { contrastRatio, wcagBadge } from '$lib/contrast';
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

  const bodyBadge = $derived(wcagBadge(contrastRatio(theme.fg, theme.bg)));
  const primaryBadge = $derived(wcagBadge(contrastRatio(theme.primaryFg, theme.primary)));
  const blocked = $derived(bodyBadge.level === 'fail');

  const colorFields: Array<[string, keyof Theme]> = [
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
    page. Body-text contrast is checked on save (WCAG AA = 4.5:1).
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" />
    <span>{form.error}</span>
  </div>
{:else if form?.saved}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" />
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

    <div class="space-y-2 pt-2">
      <div class="flex items-center justify-between text-xs">
        <span class="text-[var(--sp-muted-fg)]">Body text contrast</span>
        <span class="rounded px-2 py-0.5 font-medium {badgeClass(bodyBadge.level)}">
          {bodyBadge.label}
        </span>
      </div>
      <div class="flex items-center justify-between text-xs">
        <span class="text-[var(--sp-muted-fg)]">Primary button contrast</span>
        <span class="rounded px-2 py-0.5 font-medium {badgeClass(primaryBadge.level)}">
          {primaryBadge.label}
        </span>
      </div>
    </div>

    <div class="flex items-center gap-3 pt-2">
      <button
        type="submit"
        disabled={blocked}
        class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-50"
        style="background: var(--sp-primary); color: var(--sp-primary-fg);"
      >
        <Save size="14" /> Save
      </button>
      <button
        type="submit"
        formaction="?/reset"
        class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm"
      >
        <RotateCcw size="14" /> Reset
      </button>
    </div>
    {#if blocked}
      <p class="text-xs text-red-600">
        Save is disabled until body-text contrast reaches 4.5:1 (WCAG AA).
      </p>
    {/if}
  </form>

  <!-- Live preview -->
  <section
    class="border p-6 shadow-sm"
    style="
      background: {theme.bg};
      color: {theme.fg};
      border-color: {theme.border};
      border-radius: {theme.radius};
    "
  >
    <p class="mb-2 text-xs tracking-widest uppercase" style="color: {theme.mutedFg}">
      Live preview
    </p>
    <h2 class="text-2xl font-semibold">{theme.brandName || 'skill-pool'}</h2>
    <p class="mt-1 text-sm" style="color: {theme.mutedFg}">
      Pretend this is the catalog page on the tenant's portal.
    </p>

    <button
      type="button"
      class="mt-4 px-4 py-2 text-sm font-medium"
      style="background: {theme.primary}; color: {theme.primaryFg}; border-radius: {theme.radius}"
    >
      Primary action
    </button>

    <div
      class="mt-5 p-3 text-sm"
      style="background: {theme.muted}; color: {theme.fg}; border-radius: {theme.radius}"
    >
      Card with a muted background — that's where catalog rows render.
    </div>

    <span
      class="mt-4 inline-block rounded-full px-2 py-1 text-xs"
      style="background: {theme.accent}; color: {theme.bg};"
    >
      accent tag
    </span>
  </section>
</div>
