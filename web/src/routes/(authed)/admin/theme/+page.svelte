<script lang="ts">
  import { untrack } from 'svelte';
  import {
    CheckCircle2,
    AlertTriangle,
    RotateCcw,
    Save,
    Tag,
    Download,
    Image as ImageIcon,
    Trash2,
    Type as TypeIcon,
    Code2,
  } from '@lucide/svelte';
  import { contrastRatio, wcagBadge, checkThemeContrast } from '$lib/contrast';
  import type { Theme } from '$lib/theme';

  let { data, form } = $props();

  // The form action returns `hasLogo: true|false`; the page load also supplies
  // it. Prefer the latest form payload so the UI reflects the post-action
  // state without an extra reload.
  const hasLogo = $derived<boolean>(
    typeof form?.hasLogo === 'boolean' ? form.hasLogo : data.hasLogo,
  );
  const hasFavicon = $derived<boolean>(
    typeof form?.hasFavicon === 'boolean' ? form.hasFavicon : data.hasFavicon,
  );
  // Cache-bust the logo / favicon images after upload so new bytes show.
  const logoBust = $derived(form?.savedLogo ? Date.now() : 0);
  const faviconBust = $derived(form?.savedFavicon ? Date.now() : 0);

  // Editable copy. Intentionally non-reactive — `untrack` tells the compiler
  // we mean it. The `$effect` below re-syncs after a successful save.
  let theme = $state<Theme>(untrack(() => ({ ...(form?.draft ?? form?.theme ?? data.theme) })));

  // Custom CSS overlay editor. Pre-populated from the server on load; the
  // server returns the raw bytes verbatim so a tenant's comments survive
  // round-trips. The textarea is untracked initially and re-synced on
  // successful save/remove via the effect below.
  let customCss = $state<string>(
    untrack(() => (typeof form?.customCss === 'string' ? form.customCss : (data.customCss ?? ''))),
  );
  $effect(() => {
    if (typeof form?.customCss === 'string') {
      customCss = form.customCss;
    }
  });
  const hasCustomCss = $derived(customCss.trim().length > 0);

  // The font-picker value is bound separately because the form encodes
  // `"system"` for the OS stack but the Theme object stores `undefined` in
  // that case (so themeToCss produces the correct fallback stack).
  let fontChoice = $state<string>(untrack(() => theme.fontFamily ?? 'system'));
  $effect(() => {
    // Keep the Theme.fontFamily in sync with the picker so the live preview
    // updates without a save round-trip.
    theme.fontFamily = fontChoice === 'system' ? undefined : fontChoice;
  });

  /**
   * URL of the Google Fonts stylesheet for the picked family. `null` for the
   * system stack — we never inject `<link>` tags for the OS default.
   */
  const fontStylesheetUrl = $derived(
    fontChoice && fontChoice !== 'system'
      ? `https://fonts.googleapis.com/css2?family=${encodeURIComponent(fontChoice).replace(/%20/g, '+')}:wght@400;500;600;700&display=swap`
      : null,
  );

  $effect(() => {
    if (form?.theme && form?.saved) {
      theme = { ...form.theme };
    }
  });

  // --- Contrast badges (live, derived from current palette) ---
  const bodyBadge = $derived(wcagBadge(contrastRatio(theme.fg, theme.bg)));
  const primaryBadge = $derived(wcagBadge(contrastRatio(theme.primaryFg, theme.primary)));
  const mutedBadge = $derived(wcagBadge(contrastRatio(theme.mutedFg, theme.muted)));
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
  type RequiredStringKey =
    | 'primary'
    | 'primaryFg'
    | 'accent'
    | 'bg'
    | 'fg'
    | 'muted'
    | 'mutedFg'
    | 'border'
    | 'radius'
    | 'brandName';
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

<!-- When the user picks a Google Fonts family, drop a stylesheet `<link>` in
     the document head so the admin preview renders with the real face. The
     server-rendered portal does the same for the picked family at runtime;
     this `<svelte:head>` makes the picker live-update before save. -->
<svelte:head>
  {#if fontStylesheetUrl}
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin="anonymous" />
    <link rel="stylesheet" href={fontStylesheetUrl} />
  {/if}
</svelte:head>

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

<!-- Logo upload (server sanitizes SVG; raster formats are magic-checked).
     This sits above the colour pickers because the logo is the most visible
     bit of branding and admins will reach for it first. -->
<section class="mb-6 rounded-[var(--sp-radius)] border border-[var(--sp-border)] p-4">
  <header class="mb-3 flex items-center gap-2">
    <ImageIcon size="16" aria-hidden="true" />
    <h2 class="text-sm font-semibold">Brand logo</h2>
  </header>
  <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
    SVG, PNG, JPEG, or WEBP. Max 256&nbsp;KiB. SVGs are sanitized server-side — <code
      >&lt;script&gt;</code
    >, event handlers, off-origin
    <code>xlink:href</code>, and CSS escapes are rejected.
  </p>

  {#if hasLogo}
    <div class="mb-3 flex items-center gap-4">
      <img
        src="/admin/theme/logo{logoBust ? `?v=${logoBust}` : ''}"
        alt="Current tenant logo"
        class="h-12 w-12 rounded border border-[var(--sp-border)] bg-[var(--sp-bg)] object-contain p-1"
      />
      <form method="POST" action="?/removeLogo">
        <button
          type="submit"
          class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-red-300 px-3 py-1.5 text-xs text-red-700 hover:bg-red-50"
        >
          <Trash2 size="12" aria-hidden="true" /> Remove logo
        </button>
      </form>
    </div>
  {/if}

  {#if form?.savedLogo}
    <p class="mb-2 text-xs text-emerald-700">Logo uploaded.</p>
  {/if}
  {#if form?.removedLogo}
    <p class="mb-2 text-xs text-emerald-700">Logo removed.</p>
  {/if}

  <form
    method="POST"
    action="?/logo"
    enctype="multipart/form-data"
    class="flex flex-wrap items-center gap-3"
  >
    <input
      type="file"
      name="logo"
      accept="image/svg+xml,image/png,image/jpeg,image/webp"
      required
      aria-label="Choose logo file"
      class="text-xs"
    />
    <button
      type="submit"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-3 py-1.5 text-xs font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Upload
    </button>
  </form>
</section>

<!-- Favicon upload. Same sanitization pipeline as the logo plus `image/x-icon`.
     Smaller 64 KiB cap to nudge admins toward sensibly-sized assets. When no
     favicon is uploaded the public GET /v1/theme/favicon transparently serves
     the logo bytes, so this section is optional. -->
<section class="mb-6 rounded-[var(--sp-radius)] border border-[var(--sp-border)] p-4">
  <header class="mb-3 flex items-center gap-2">
    <ImageIcon size="16" aria-hidden="true" />
    <h2 class="text-sm font-semibold">Favicon</h2>
  </header>
  <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
    SVG, PNG, JPEG, WEBP, or ICO. Max 64&nbsp;KiB. If you don't upload one, the brand logo is served
    at the favicon URL as a fallback.
  </p>

  {#if hasFavicon}
    <div class="mb-3 flex items-center gap-4">
      <img
        src="/admin/theme/favicon{faviconBust ? `?v=${faviconBust}` : ''}"
        alt="Current tenant favicon"
        class="h-8 w-8 rounded border border-[var(--sp-border)] bg-[var(--sp-bg)] object-contain p-0.5"
      />
      <form method="POST" action="?/removeFavicon">
        <button
          type="submit"
          class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-red-300 px-3 py-1.5 text-xs text-red-700 hover:bg-red-50"
        >
          <Trash2 size="12" aria-hidden="true" /> Remove favicon
        </button>
      </form>
    </div>
  {:else if hasLogo}
    <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
      No favicon uploaded — the logo is served at the favicon URL.
    </p>
  {/if}

  {#if form?.savedFavicon}
    <p class="mb-2 text-xs text-emerald-700">Favicon uploaded.</p>
  {/if}
  {#if form?.removedFavicon}
    <p class="mb-2 text-xs text-emerald-700">Favicon removed.</p>
  {/if}

  <form
    method="POST"
    action="?/favicon"
    enctype="multipart/form-data"
    class="flex flex-wrap items-center gap-3"
  >
    <input
      type="file"
      name="favicon"
      accept="image/svg+xml,image/png,image/jpeg,image/webp,image/x-icon,image/vnd.microsoft.icon,.ico"
      required
      aria-label="Choose favicon file"
      class="text-xs"
    />
    <button
      type="submit"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-3 py-1.5 text-xs font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Upload
    </button>
  </form>
</section>

<!-- Custom CSS overlay. The server runs a strict sanitizer over the bytes
     before persisting; the response also pins
     `Content-Security-Policy: style-src 'self'` so a sanitizer bypass can't
     pull in an external sheet. 32 KiB cap. -->
<section class="mb-6 rounded-[var(--sp-radius)] border border-[var(--sp-border)] p-4">
  <header class="mb-3 flex items-center gap-2">
    <Code2 size="16" aria-hidden="true" />
    <h2 class="text-sm font-semibold">Custom CSS overlay</h2>
  </header>
  <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
    Layer a CSS overlay on top of the curated <code>--sp-*</code> variables. Max 32&nbsp;KiB. The
    server rejects <code>@import</code>, off-site
    <code>url()</code>, <code>expression()</code>, <code>behavior:</code>,
    <code>javascript:</code> URIs, and HTML-tag-like content — see the enterprise docs for the full list.
  </p>

  {#if form?.savedCustomCss}
    <p class="mb-2 text-xs text-emerald-700">Custom CSS saved.</p>
  {/if}
  {#if form?.removedCustomCss}
    <p class="mb-2 text-xs text-emerald-700">Custom CSS removed.</p>
  {/if}

  <form method="POST" action="?/customCss" class="space-y-3">
    <textarea
      name="customCss"
      bind:value={customCss}
      rows="12"
      placeholder={'.sp-hero { background: var(--sp-primary); }'}
      class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-xs leading-relaxed"
      spellcheck="false"
      aria-label="Custom CSS overlay editor"
    ></textarea>
    <div class="flex flex-wrap items-center gap-3">
      <button
        type="submit"
        class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-3 py-1.5 text-xs font-medium"
        style="background: var(--sp-primary); color: var(--sp-primary-fg);"
      >
        <Save size="12" aria-hidden="true" /> Save custom CSS
      </button>
      {#if hasCustomCss}
        <button
          type="submit"
          formaction="?/removeCustomCss"
          class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-red-300 px-3 py-1.5 text-xs text-red-700 hover:bg-red-50"
        >
          <Trash2 size="12" aria-hidden="true" /> Remove custom CSS
        </button>
      {/if}
      <span class="text-xs text-[var(--sp-muted-fg)]">
        {new Blob([customCss]).size} / 32&thinsp;768 bytes
      </span>
    </div>
  </form>
</section>

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

    <!-- Font picker — populated from /v1/theme/fonts so the client list is
         never out of sync with the server's `ALLOWED_FONTS`. The `"system"`
         choice is stored server-side as NULL and resolved by `themeToCss`
         into the OS-native font stack. -->
    <label class="flex items-start justify-between gap-3 pt-1">
      <span class="text-sm text-[var(--sp-fg)]">
        <span class="inline-flex items-center gap-2">
          <TypeIcon size="14" aria-hidden="true" /> Font family
        </span>
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          Curated for performance + permissive licences. Pick <code>system</code>
          to inherit the OS stack and keep the page weight zero.
        </span>
      </span>
      <select
        name="fontFamily"
        bind:value={fontChoice}
        class="w-48 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 text-sm"
      >
        {#each data.fonts as font (font)}
          <option value={font}>{font}</option>
        {/each}
      </select>
    </label>

    {#if fontChoice && fontChoice !== 'system'}
      <p class="-mt-1 text-xs text-[var(--sp-muted-fg)]">
        The portal loads <strong>{fontChoice}</strong> from Google Fonts on every page. Want to
        self-host? Pick <code>system</code> here and add your own
        <code>@font-face</code> declarations in a custom-CSS layer.
      </p>
    {/if}

    <label class="flex items-center justify-between gap-3">
      <span class="text-sm text-[var(--sp-fg)]">
        Show "Powered by skill-pool" footer
        <span class="block text-xs text-[var(--sp-muted-fg)]"
          >Default on (Free tier). Uncheck to hide the footer credit.</span
        >
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
    <p class="mb-3 text-xs font-medium tracking-widest text-[var(--sp-muted-fg)] uppercase">
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
          <h2 class="truncate text-sm leading-snug font-semibold" style="color: {theme.fg};">
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
        <span class="text-xs" style="color: {theme.mutedFg};"> Used 142 times </span>
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
