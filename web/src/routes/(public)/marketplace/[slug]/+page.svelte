<script lang="ts">
  import { Copy, Package } from '@lucide/svelte';
  import type { PluginContent } from '$lib/server/api';

  let { data } = $props();

  const plugin = $derived(data.plugin);
  const versions = $derived(data.versions);

  const skillContents = $derived(plugin.contents.filter((c: PluginContent) => c.kind === 'skill'));
  const agentContents = $derived(plugin.contents.filter((c: PluginContent) => c.kind === 'agent'));
  const commandContents = $derived(
    plugin.contents.filter((c: PluginContent) => c.kind === 'command'),
  );

  const manifestJson = $derived(JSON.stringify(plugin.manifest, null, 2));

  const installCommand = $derived(`/plugin marketplace add ${data.installBase}`);

  let commandCopied = $state(false);

  async function copyInstallCommand() {
    try {
      await navigator.clipboard?.writeText(installCommand);
      commandCopied = true;
      setTimeout(() => (commandCopied = false), 1500);
    } catch {
      commandCopied = false;
    }
  }

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

<svelte:head>
  <title>{plugin.name || plugin.slug} · Plugin Marketplace</title>
  <meta
    name="description"
    content={plugin.description ?? `${plugin.name || plugin.slug} plugin for Claude Code`}
  />
  <!-- JSON-LD structured data for SEO (SoftwareApplication) -->
  {@html `<script type="application/ld+json">${data.jsonLd}<\/script>`}
</svelte:head>

<header class="border-b border-[var(--sp-border)] bg-[var(--sp-bg)] px-6 py-6 md:px-12">
  <div class="mx-auto max-w-4xl">
    <!-- Breadcrumb -->
    <nav aria-label="Breadcrumb" class="mb-3 text-xs text-[var(--sp-muted-fg)]">
      <a href="/marketplace" class="hover:underline">Marketplace</a>
      <span class="mx-1" aria-hidden="true">/</span>
      <span class="font-mono">{plugin.slug}</span>
    </nav>

    <div class="flex items-start gap-3">
      <Package size="24" class="mt-1 shrink-0 text-[var(--sp-primary)]" aria-hidden="true" />
      <div>
        <h1 class="text-2xl font-semibold text-[var(--sp-fg)]">
          {plugin.name || plugin.slug}
        </h1>
        <p
          class="mt-1 flex flex-wrap items-center gap-2 font-mono text-xs text-[var(--sp-muted-fg)]"
        >
          <span>{plugin.slug}</span>
          <span aria-hidden="true">·</span>
          <span>v{plugin.version}</span>
          <span aria-hidden="true">·</span>
          <span
            class="rounded-full border border-[var(--sp-border)] bg-[var(--sp-muted)] px-2 py-0.5 text-[var(--sp-fg)]"
          >
            {plugin.sourcing_mode}
          </span>
          {#if plugin.updated_at}
            <span aria-hidden="true">·</span>
            <span>updated {fmtDate(plugin.updated_at)}</span>
          {/if}
        </p>

        {#if plugin.description}
          <p class="mt-2 text-sm text-[var(--sp-muted-fg)]">{plugin.description}</p>
        {/if}
      </div>
    </div>
  </div>
</header>

<main class="px-6 py-8 md:px-12">
  <div class="mx-auto max-w-4xl space-y-8">
    <!-- Install CTA -->
    <section aria-labelledby="install-heading">
      <h2
        id="install-heading"
        class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
      >
        Install
      </h2>
      <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
        Run this command inside a Claude Code session to add this marketplace, then install the
        plugin:
      </p>
      <div class="flex items-stretch gap-2">
        <input
          type="text"
          readonly
          value={installCommand}
          aria-label="Install command"
          class="flex-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-2 font-mono text-xs text-[var(--sp-fg)]"
        />
        <button
          type="button"
          onclick={copyInstallCommand}
          aria-label="Copy install command"
          class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-xs font-medium text-[var(--sp-fg)] hover:border-[var(--sp-primary)]"
        >
          <Copy size="12" aria-hidden="true" />
          {commandCopied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <p class="mt-2 text-xs text-[var(--sp-muted-fg)]">
        Then run:
        <code class="rounded bg-[var(--sp-muted)] px-1">
          /plugin install {plugin.slug}@{data.installBase.replace(/^https?:\/\//, '').split('.')[0]}
        </code>
      </p>
    </section>

    <!-- Contents -->
    <section aria-labelledby="contents-heading">
      <h2
        id="contents-heading"
        class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
      >
        Contents · {plugin.contents.length}
      </h2>

      <div class="grid grid-cols-1 gap-4 md:grid-cols-3">
        {#each [{ title: 'Skills', items: skillContents, kind: 'skill' as const }, { title: 'Agents', items: agentContents, kind: 'agent' as const }, { title: 'Commands', items: commandContents, kind: 'command' as const }] as group (group.kind)}
          <div
            class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]"
          >
            <header
              class="border-b border-[var(--sp-border)] px-3 py-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
            >
              {group.title} · {group.items.length}
            </header>
            {#if group.items.length === 0}
              <p class="px-3 py-3 text-xs text-[var(--sp-muted-fg)]">(none)</p>
            {:else}
              <ul class="divide-y divide-[var(--sp-border)] text-xs">
                {#each group.items as item (item.slug)}
                  <li class="px-3 py-2 font-mono">
                    <span class="text-[var(--sp-fg)]">{item.slug}</span>
                    <span class="ml-1 text-[var(--sp-muted-fg)]">@ {item.version}</span>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>
        {/each}
      </div>
    </section>

    <!-- Manifest -->
    <section aria-labelledby="manifest-heading">
      <h2
        id="manifest-heading"
        class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
      >
        Manifest
      </h2>
      <pre
        class="max-h-72 overflow-auto rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-3 font-mono text-xs text-[var(--sp-fg)]"
        aria-label="Plugin manifest JSON">{manifestJson}</pre>
    </section>

    <!-- Version history -->
    {#if versions.length > 0}
      <section aria-labelledby="versions-heading">
        <h2
          id="versions-heading"
          class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
        >
          Version history
        </h2>
        <div class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]">
          <table class="w-full text-xs">
            <thead
              class="bg-[var(--sp-muted)] text-left tracking-wide text-[var(--sp-muted-fg)] uppercase"
            >
              <tr>
                <th class="px-3 py-2 font-medium" scope="col">Version</th>
                <th class="px-3 py-2 font-medium" scope="col">Status</th>
                <th class="px-3 py-2 font-medium" scope="col">Published</th>
              </tr>
            </thead>
            <tbody class="divide-y divide-[var(--sp-border)] bg-[var(--sp-bg)]">
              {#each versions as v (v.version)}
                <tr>
                  <td class="px-3 py-2 font-mono">v{v.version}</td>
                  <td class="px-3 py-2">
                    <span
                      class="rounded-full border border-[var(--sp-border)] bg-[var(--sp-muted)] px-1.5 py-0.5 text-[10px] text-[var(--sp-muted-fg)]"
                    >
                      {v.status}
                    </span>
                  </td>
                  <td class="px-3 py-2 text-[var(--sp-muted-fg)]">{fmtDate(v.created_at)}</td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </section>
    {/if}
  </div>
</main>
