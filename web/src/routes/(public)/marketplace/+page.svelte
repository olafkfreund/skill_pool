<script lang="ts">
  import { AlertTriangle, Copy, Package, Tag } from '@lucide/svelte';

  let { data } = $props();

  // Per-card copy state: keyed by plugin slug.
  let copiedSlug = $state<string | null>(null);

  function installCommand(installBase: string): string {
    return `/plugin marketplace add ${installBase}`;
  }

  async function copyInstall(slug: string) {
    try {
      await navigator.clipboard?.writeText(installCommand(data.installBase));
      copiedSlug = slug;
      setTimeout(() => {
        if (copiedSlug === slug) copiedSlug = null;
      }, 1500);
    } catch {
      copiedSlug = null;
    }
  }
</script>

<svelte:head>
  <title>{data.installBase.replace(/^https?:\/\//, '')} · Plugin Marketplace</title>
  <meta name="robots" content="index, follow" />
</svelte:head>

<!-- Page header -->
<header class="border-b border-[var(--sp-border)] bg-[var(--sp-bg)] px-6 py-8 md:px-12">
  <div class="mx-auto max-w-6xl">
    <div class="flex items-center gap-3">
      <Package size="28" class="shrink-0 text-[var(--sp-primary)]" />
      <div>
        <h1 class="text-2xl font-semibold text-[var(--sp-fg)]">Plugin Marketplace</h1>
        <p class="mt-0.5 text-sm text-[var(--sp-muted-fg)]">
          Browse and install plugins for Claude Code. No account required.
        </p>
      </div>
    </div>

    <!-- Tenant install command -->
    <div class="mt-6 max-w-xl">
      <p class="mb-1.5 text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">
        Add this marketplace to Claude Code
      </p>
      <div class="flex items-stretch gap-2">
        <code
          class="flex-1 overflow-x-auto rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-2 text-xs text-[var(--sp-fg)]"
          aria-label="Marketplace install command"
        >
          {installCommand(data.installBase)}
        </code>
        <button
          type="button"
          onclick={() => copyInstall('__header__')}
          aria-label="Copy marketplace install command"
          class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-xs font-medium text-[var(--sp-fg)] hover:border-[var(--sp-primary)]"
        >
          <Copy size="12" />
          {copiedSlug === '__header__' ? 'Copied' : 'Copy'}
        </button>
      </div>
    </div>
  </div>
</header>

<main class="px-6 py-8 md:px-12">
  <div class="mx-auto max-w-6xl">
    {#if 'error' in data && data.error}
      <div
        class="mb-6 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
        role="alert"
      >
        <AlertTriangle size="16" class="mt-0.5 shrink-0" aria-hidden="true" />
        <span>{data.error}</span>
      </div>
    {/if}

    {#if data.plugins.length === 0}
      <div
        class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-16 text-center"
      >
        <Package size="32" class="mx-auto mb-3 text-[var(--sp-muted-fg)]" aria-hidden="true" />
        <p class="text-sm text-[var(--sp-muted-fg)]">
          No plugins have been published to this marketplace yet.
        </p>
      </div>
    {:else}
      <ul class="grid grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3" aria-label="Plugin list">
        {#each data.plugins as plugin (plugin.slug)}
          <li class="flex flex-col">
            <article
              class="flex h-full flex-col rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)]"
            >
              <!-- Card header: name + version -->
              <div class="flex-1 px-4 pt-4 pb-3">
                <div class="flex items-start justify-between gap-2">
                  <h2 class="leading-snug font-semibold text-[var(--sp-fg)]">
                    <a
                      href={`/marketplace/${encodeURIComponent(plugin.slug)}`}
                      class="hover:underline focus:underline focus:outline-none"
                    >
                      {plugin.name || plugin.slug}
                    </a>
                  </h2>
                  <span
                    class="shrink-0 rounded-full border border-[var(--sp-border)] bg-[var(--sp-muted)] px-2 py-0.5 font-mono text-[10px] text-[var(--sp-muted-fg)]"
                  >
                    v{plugin.version}
                  </span>
                </div>

                <!-- Description (2-line clamp via line-clamp utility) -->
                {#if plugin.description}
                  <p
                    class="mt-1.5 line-clamp-2 text-sm text-[var(--sp-muted-fg)]"
                    title={plugin.description}
                  >
                    {plugin.description}
                  </p>
                {/if}

                <!-- Tags -->
                {#if plugin.tags && plugin.tags.length > 0}
                  <div class="mt-2 flex flex-wrap items-center gap-1" aria-label="Tags">
                    <Tag size="10" class="shrink-0 text-[var(--sp-muted-fg)]" aria-hidden="true" />
                    {#each plugin.tags.slice(0, 4) as tag (tag)}
                      <span
                        class="rounded-full bg-[var(--sp-muted)] px-1.5 py-0.5 font-mono text-[10px] text-[var(--sp-muted-fg)]"
                      >
                        {tag}
                      </span>
                    {/each}
                    {#if plugin.tags.length > 4}
                      <span class="text-[10px] text-[var(--sp-muted-fg)]">
                        +{plugin.tags.length - 4}
                      </span>
                    {/if}
                  </div>
                {/if}
              </div>

              <!-- Card footer: view detail + copy install -->
              <footer
                class="flex items-center justify-between gap-2 border-t border-[var(--sp-border)] px-4 py-2"
              >
                <a
                  href={`/marketplace/${encodeURIComponent(plugin.slug)}`}
                  class="text-xs text-[var(--sp-primary)] hover:underline focus:underline focus:outline-none"
                >
                  View details
                </a>
                <button
                  type="button"
                  onclick={() => copyInstall(plugin.slug)}
                  aria-label={`Copy install command for ${plugin.name || plugin.slug}`}
                  class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 text-[11px] font-medium text-[var(--sp-fg)] hover:border-[var(--sp-primary)]"
                >
                  <Copy size="10" aria-hidden="true" />
                  {copiedSlug === plugin.slug ? 'Copied!' : 'Copy install'}
                </button>
              </footer>
            </article>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</main>
