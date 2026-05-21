<script lang="ts">
  import { untrack } from 'svelte';
  import { AlertTriangle, CheckCircle2, Copy, Package, RefreshCw, Trash2 } from '@lucide/svelte';
  import type { PluginContent, PluginVersionRow } from '$lib/server/api';

  let { data, form } = $props();

  const plugin = $derived(data.plugin);
  const versions = $derived(data.versions as PluginVersionRow[]);
  const isCurator = $derived(data.userRole === 'curator' || data.userRole === 'admin');
  const isMirror = $derived(plugin.sourcing_mode === 'mirror');
  const canManageMirror = $derived(isCurator && isMirror);

  const skillContents = $derived(plugin.contents.filter((c: PluginContent) => c.kind === 'skill'));
  const agentContents = $derived(plugin.contents.filter((c: PluginContent) => c.kind === 'agent'));
  const commandContents = $derived(
    plugin.contents.filter((c: PluginContent) => c.kind === 'command'),
  );

  const manifestJson = $derived(JSON.stringify(plugin.manifest, null, 2));

  let copied = $state(false);

  /**
   * Copy the public marketplace URL. Falls back to a no-op when the
   * Clipboard API is unavailable (older browsers, http://) — the URL
   * itself is always visible in the input so the user can copy manually.
   */
  async function copyMarketplaceUrl() {
    try {
      await navigator.clipboard?.writeText(data.marketplaceUrl);
      copied = true;
      setTimeout(() => (copied = false), 1500);
    } catch {
      copied = false;
    }
  }

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

  // Auto-refresh form state — only meaningful when sourcing_mode === 'mirror'.
  // Default to 24h (the doc'd default) when the toggle is first enabled.
  let autoRefreshEnabled = $state(false);
  let autoRefreshInterval = $state(untrack(() => 86_400));

  // The form prop is the discriminated union returned by the action;
  // tracking_issue lives only on the setAutoRefresh 503 branch. Widen
  // to a record so we can read both branches without leaning on
  // generated SvelteKit types we'd duplicate here.
  const formAny = $derived(form as Record<string, unknown> | null);
  const lastAction = $derived(formAny?.action as string | undefined);
  const versionArchived = $derived(lastAction === 'archiveVersion' && !!formAny?.archived);
  const autoRefreshError = $derived(lastAction === 'setAutoRefresh' && !!formAny?.error);
  const trackingIssue = $derived(
    typeof formAny?.tracking_issue === 'number' ? (formAny.tracking_issue as number) : undefined,
  );
  const hasError = $derived(!!formAny?.error && !autoRefreshError);
</script>

<header class="mb-6">
  <nav class="mb-2 text-xs text-[var(--sp-muted-fg)]">
    <a href="/admin/plugins" class="hover:underline">Plugins</a>
    <span class="mx-1">/</span>
    <span class="font-mono">{plugin.slug}</span>
  </nav>
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Package size="22" />
    {plugin.name}
  </h1>
  <p class="mt-1 flex flex-wrap items-center gap-2 font-mono text-xs text-[var(--sp-muted-fg)]">
    <span>{plugin.slug}</span>
    <span>·</span>
    <span>v{plugin.version}</span>
    <span>·</span>
    <span
      class="rounded-full border border-[var(--sp-border)] bg-[var(--sp-muted)] px-2 py-0.5 text-[var(--sp-fg)]"
    >
      {plugin.sourcing_mode}
    </span>
    <span>·</span>
    <span>updated {fmtDate(plugin.updated_at)}</span>
  </p>
</header>

{#if hasError}
  <div
    class="mb-6 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{formAny?.error as string}</span>
  </div>
{:else if versionArchived}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Archived version
    <code class="rounded bg-emerald-100 px-1">v{formAny?.version as string}</code>.
  </div>
{/if}

<!-- ─── Public marketplace URL ──────────────────────────────────────────── -->
<section class="mb-8 max-w-3xl">
  <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Marketplace URL
  </h2>
  <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
    Paste this into Claude Code:
    <code class="rounded bg-[var(--sp-muted)] px-1">/plugin marketplace add &lt;url&gt;</code>.
    Public read, no auth.
  </p>
  <div class="flex items-stretch gap-2">
    <input
      type="text"
      readonly
      value={data.marketplaceUrl}
      aria-label="Marketplace URL"
      class="flex-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-2 font-mono text-xs text-[var(--sp-fg)]"
    />
    <button
      type="button"
      onclick={copyMarketplaceUrl}
      aria-label="Copy marketplace URL"
      class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-2 text-xs font-medium hover:border-[var(--sp-primary)]"
    >
      <Copy size="12" />
      {copied ? 'Copied' : 'Copy'}
    </button>
  </div>
</section>

<!-- ─── Manifest preview ────────────────────────────────────────────────── -->
<section class="mb-8 max-w-3xl">
  <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Manifest
  </h2>
  <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
    Rendered as it would land in
    <code class="rounded bg-[var(--sp-muted)] px-1">.claude-plugin/plugin.json</code>.
  </p>
  <pre
    class="max-h-96 overflow-auto rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-3 font-mono text-xs text-[var(--sp-fg)]">{manifestJson}</pre>
</section>

<!-- ─── Contents ────────────────────────────────────────────────────────── -->
<section class="mb-8">
  <h2 class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Contents · {plugin.contents.length}
  </h2>

  <div class="grid grid-cols-1 gap-4 md:grid-cols-3">
    {#each [{ title: 'Skills', items: skillContents, kind: 'skill' as const }, { title: 'Agents', items: agentContents, kind: 'agent' as const }, { title: 'Commands', items: commandContents, kind: 'command' as const }] as group (group.kind)}
      <div class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]">
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
                <a
                  href={`/skills/${encodeURIComponent(item.slug)}`}
                  class="text-[var(--sp-fg)] hover:underline"
                >
                  {item.slug}
                </a>
                <span class="ml-1 text-[var(--sp-muted-fg)]">@ {item.version}</span>
              </li>
            {/each}
          </ul>
        {/if}
      </div>
    {/each}
  </div>
</section>

<!-- ─── Version history ─────────────────────────────────────────────────── -->
<section class="mb-8 max-w-3xl">
  <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Version history
  </h2>
  {#if versions.length === 0}
    <p class="text-xs text-[var(--sp-muted-fg)]">No version history available.</p>
  {:else}
    <div class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]">
      <table class="w-full text-xs">
        <thead
          class="bg-[var(--sp-muted)] text-left tracking-wide text-[var(--sp-muted-fg)] uppercase"
        >
          <tr>
            <th class="px-3 py-2 font-medium">Version</th>
            <th class="px-3 py-2 font-medium">Status</th>
            <th class="px-3 py-2 font-medium">Created</th>
            <th class="px-3 py-2 font-medium">By</th>
            <th class="px-3 py-2 text-right font-medium">Action</th>
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
              <td class="px-3 py-2 text-[var(--sp-muted-fg)]">{v.published_by ?? '—'}</td>
              <td class="px-3 py-2 text-right">
                {#if isCurator && v.status !== 'archived'}
                  <form
                    method="POST"
                    action="?/archiveVersion"
                    class="inline-block"
                    onsubmit={(e) => {
                      if (
                        !confirm(
                          `Archive ${plugin.slug}@${v.version}? It stops appearing in the marketplace.`,
                        )
                      ) {
                        e.preventDefault();
                      }
                    }}
                  >
                    <input type="hidden" name="version" value={v.version} />
                    <button
                      type="submit"
                      class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-[10px] font-medium text-red-700 hover:bg-red-50"
                    >
                      <Trash2 size="10" /> Archive
                    </button>
                  </form>
                {:else}
                  <span class="text-[10px] text-[var(--sp-muted-fg)]">{v.status}</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<!-- ─── Mirror auto-refresh toggle (curator / admin, sourcing_mode = mirror only) ─── -->
{#if canManageMirror}
  <section class="mb-8 max-w-3xl">
    <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
      Mirror auto-refresh
    </h2>
    <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
      How often skill-pool re-pulls
      <code class="rounded bg-[var(--sp-muted)] px-1"
        >{plugin.upstream_url ?? plugin.external_git_url ?? '(upstream)'}</code
      >
      into the local mirror. Default 24h.
    </p>

    {#if autoRefreshError}
      <div
        class="mb-3 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
      >
        <AlertTriangle size="16" class="mt-0.5 shrink-0" />
        <span>
          {formAny?.error as string}
          {#if trackingIssue}
            <a
              href={`https://github.com/olafkfreund/skill_pool/issues/${trackingIssue}`}
              target="_blank"
              rel="noopener noreferrer"
              class="ml-1 underline"
            >
              issue #{trackingIssue} ↗
            </a>
          {/if}
        </span>
      </div>
    {/if}

    <form
      method="POST"
      action="?/setAutoRefresh"
      class="space-y-3 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-4 py-3"
    >
      <label class="flex cursor-pointer items-center gap-2 text-sm font-medium text-[var(--sp-fg)]">
        <input
          type="checkbox"
          name="auto_refresh_enabled"
          bind:checked={autoRefreshEnabled}
          class="h-4 w-4 rounded border-[var(--sp-border)]"
        />
        Auto-refresh from upstream
      </label>

      {#if autoRefreshEnabled}
        <div class="flex items-center gap-2">
          <label class="text-xs text-[var(--sp-muted-fg)]" for="interval_secs">
            Interval (seconds, min 300):
          </label>
          <input
            id="interval_secs"
            type="number"
            name="interval_secs"
            bind:value={autoRefreshInterval}
            min="300"
            step="1"
            placeholder="86400"
            class="w-28 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
          />
          <span class="text-xs text-[var(--sp-muted-fg)]">
            {#if autoRefreshInterval >= 86400}
              ({Math.round(autoRefreshInterval / 86400)}d)
            {:else if autoRefreshInterval >= 3600}
              ({Math.round(autoRefreshInterval / 3600)}h)
            {:else}
              ({Math.round(autoRefreshInterval / 60)}m)
            {/if}
          </span>
        </div>
      {/if}

      <button
        type="submit"
        class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-1.5 text-xs font-medium hover:border-[var(--sp-primary)]"
      >
        <RefreshCw size="11" /> Save schedule
      </button>
    </form>
  </section>
{/if}
