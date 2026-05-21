<script lang="ts">
  import { untrack } from 'svelte';
  import { AlertTriangle, Package, Plus, X } from '@lucide/svelte';
  import type { Skill } from '$lib/types';

  let { data, form } = $props();

  // `form` is the discriminated union returned by the action. We widen
  // it to a record so we can read the optional echo fields without
  // duplicating SvelteKit's generated PageData types here.
  const formAny = $derived(form as Record<string, unknown> | null);

  // ---- Manifest fields ---------------------------------------------------
  // Seed each field once from the echoed form on first render; from
  // then on the local $state owns the value. `untrack` quiets the
  // Svelte warning about initial-value capture (the capture is the
  // desired behaviour here, not a bug).

  function readEchoed(key: string, fallback = ''): string {
    const v = (form as Record<string, unknown> | null)?.[key];
    return typeof v === 'string' ? v : fallback;
  }

  let slug = $state(untrack(() => readEchoed('slug')));
  let displayName = $state(untrack(() => readEchoed('displayName')));
  let version = $state(untrack(() => readEchoed('version', '1.0.0')));
  let description = $state(untrack(() => readEchoed('description')));
  let sourcingMode = $state(
    untrack(() => {
      const v = readEchoed('sourcing_mode', 'internal');
      return v === 'external' || v === 'mirror'
        ? v
        : ('internal' as 'internal' | 'external' | 'mirror');
    }),
  );
  let externalGitUrl = $state(untrack(() => readEchoed('external_git_url')));

  let hooksJson = $state(untrack(() => readEchoed('hooks_json')));
  let mcpServersJson = $state(untrack(() => readEchoed('mcp_servers_json')));
  let lspServersJson = $state(untrack(() => readEchoed('lsp_servers_json')));
  let monitorsJson = $state(untrack(() => readEchoed('monitors_json')));

  // Which JSON section the server flagged as invalid, if any. Only
  // the inline-JSON branches of the action set `section`.
  const erroredSection = $derived(
    typeof formAny?.section === 'string' ? (formAny.section as string) : undefined,
  );

  // ---- Contents (multi-select per kind) ----------------------------------

  type Pick = { slug: string; version: string };

  /**
   * Parse the comma-separated "slug@version" strings echoed back by the
   * server on validation failure so the user doesn't lose their picks.
   * Same shape as the server-side parser.
   */
  function parseSelected(raw: string): Pick[] {
    return raw
      .split(',')
      .map((entry) => entry.trim())
      .filter(Boolean)
      .map((entry) => {
        const [s, v] = entry.split('@', 2);
        return { slug: s?.trim() ?? '', version: (v ?? '').trim() };
      })
      .filter((p) => p.slug && p.version);
  }

  let selectedSkills = $state<Pick[]>(untrack(() => parseSelected(readEchoed('selected_skills'))));
  let selectedAgents = $state<Pick[]>(untrack(() => parseSelected(readEchoed('selected_agents'))));
  let selectedCommands = $state<Pick[]>(
    untrack(() => parseSelected(readEchoed('selected_commands'))),
  );

  let skillQuery = $state('');
  let agentQuery = $state('');
  let commandQuery = $state('');

  const TOTAL_CAP = 64;
  const totalSelected = $derived(
    selectedSkills.length + selectedAgents.length + selectedCommands.length,
  );
  const overCap = $derived(totalSelected > TOTAL_CAP);

  /**
   * Filter a kind-specific catalogue against the user's query and exclude
   * items already in `selected`. Cap rendered results at 50 to keep the
   * DOM small even when the tenant has 500+ entries.
   */
  function filterCatalog(catalogue: Skill[], query: string, selected: Pick[]): Skill[] {
    const q = query.trim().toLowerCase();
    const chosen = new Set(selected.map((p) => p.slug));
    let pool = catalogue.filter((s) => !chosen.has(s.slug));
    if (q) {
      pool = pool.filter(
        (s) => s.slug.toLowerCase().includes(q) || (s.description ?? '').toLowerCase().includes(q),
      );
    }
    return pool.slice(0, 50);
  }

  const filteredSkills = $derived(filterCatalog(data.skills, skillQuery, selectedSkills));
  const filteredAgents = $derived(filterCatalog(data.agents, agentQuery, selectedAgents));
  const filteredCommands = $derived(filterCatalog(data.commands, commandQuery, selectedCommands));

  function add(kind: 'skill' | 'agent' | 'command', s: Skill) {
    const pick: Pick = { slug: s.slug, version: s.version };
    if (kind === 'skill') selectedSkills = [...selectedSkills, pick];
    if (kind === 'agent') selectedAgents = [...selectedAgents, pick];
    if (kind === 'command') selectedCommands = [...selectedCommands, pick];
  }

  function remove(kind: 'skill' | 'agent' | 'command', slug: string) {
    if (kind === 'skill') selectedSkills = selectedSkills.filter((p) => p.slug !== slug);
    if (kind === 'agent') selectedAgents = selectedAgents.filter((p) => p.slug !== slug);
    if (kind === 'command') selectedCommands = selectedCommands.filter((p) => p.slug !== slug);
  }

  /** Serialise each kind's picks as "slug@version, slug@version" for the hidden form fields. */
  function serializePicks(picks: Pick[]): string {
    return picks.map((p) => `${p.slug}@${p.version}`).join(', ');
  }
</script>

<header class="mb-6">
  <nav class="mb-2 text-xs text-[var(--sp-muted-fg)]">
    <a href="/admin/plugins" class="hover:underline">Plugins</a>
    <span class="mx-1">/</span>
    <span>New plugin</span>
  </nav>
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Package size="22" /> New plugin
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Assemble a plugin from skills, agents, and commands already in this tenant's catalogue.
  </p>
</header>

{#if formAny?.error}
  <div
    class="mb-6 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{formAny.error as string}</span>
  </div>
{/if}

<form method="POST" class="space-y-8">
  <!-- ─── Manifest ──────────────────────────────────────────────────────── -->
  <section class="max-w-2xl">
    <h2 class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
      Manifest
    </h2>

    <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">
          Slug <span class="text-red-500">*</span>
        </span>
        <input
          type="text"
          name="slug"
          required
          bind:value={slug}
          placeholder="rust-axum-toolkit"
          pattern="[a-z0-9]+(-[a-z0-9]+)*"
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          Kebab-case, 1–64 chars. Doubles as the plugin's
          <code class="rounded bg-[var(--sp-muted)] px-1">name</code> in
          <code class="rounded bg-[var(--sp-muted)] px-1">plugin.json</code>.
        </span>
      </label>

      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">
          Version <span class="text-red-500">*</span>
        </span>
        <input
          type="text"
          name="version"
          required
          bind:value={version}
          placeholder="1.0.0"
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          Semver. Bump on every release — pinned plugins ignore identical version strings.
        </span>
      </label>

      <label class="block sm:col-span-2">
        <span class="text-sm font-medium text-[var(--sp-fg)]">Display name</span>
        <input
          type="text"
          name="displayName"
          bind:value={displayName}
          placeholder="Rust + Axum Toolkit"
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          Optional. Human-readable label for the
          <code class="rounded bg-[var(--sp-muted)] px-1">/plugin</code> picker. Falls back to the slug.
        </span>
      </label>

      <label class="block sm:col-span-2">
        <span class="text-sm font-medium text-[var(--sp-fg)]">
          Description <span class="text-red-500">*</span>
        </span>
        <textarea
          name="description"
          rows="2"
          required
          bind:value={description}
          placeholder="Curated skills, agents, and hooks for Rust + Axum service development"
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        ></textarea>
      </label>
    </div>

    <fieldset class="mt-5">
      <legend class="text-sm font-medium text-[var(--sp-fg)]">Sourcing mode</legend>
      <div class="mt-2 flex flex-wrap gap-4">
        <label class="flex cursor-pointer items-center gap-2 text-sm">
          <input
            type="radio"
            name="sourcing_mode"
            value="internal"
            bind:group={sourcingMode}
            class="h-4 w-4"
          />
          Internal (composed here)
        </label>
        <label class="flex cursor-pointer items-center gap-2 text-sm">
          <input
            type="radio"
            name="sourcing_mode"
            value="external"
            bind:group={sourcingMode}
            class="h-4 w-4"
          />
          External (upstream git, no mirror)
        </label>
        <label class="flex cursor-pointer items-center gap-2 text-sm">
          <input
            type="radio"
            name="sourcing_mode"
            value="mirror"
            bind:group={sourcingMode}
            class="h-4 w-4"
          />
          Mirror (clone + serve locally)
        </label>
      </div>
    </fieldset>

    {#if sourcingMode !== 'internal'}
      <label class="mt-4 block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">
          External git URL <span class="text-red-500">*</span>
        </span>
        <input
          type="url"
          name="external_git_url"
          required
          bind:value={externalGitUrl}
          placeholder="https://github.com/acme-corp/formatter"
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
      </label>
    {/if}
  </section>

  <!-- ─── Contents (three-column picker) ──────────────────────────────── -->
  <section>
    <h2 class="mb-1 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
      Contents
    </h2>
    <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
      <span class:text-red-600={overCap} class="font-mono">{totalSelected}</span>
      of {TOTAL_CAP} max. Pick at least one item.
    </p>

    <input type="hidden" name="selected_skills" value={serializePicks(selectedSkills)} />
    <input type="hidden" name="selected_agents" value={serializePicks(selectedAgents)} />
    <input type="hidden" name="selected_commands" value={serializePicks(selectedCommands)} />

    <div class="grid grid-cols-1 gap-4 md:grid-cols-3">
      <!-- Skills column -->
      <div class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]">
        <header
          class="border-b border-[var(--sp-border)] px-3 py-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
        >
          Skills · {selectedSkills.length}
        </header>
        <div class="px-3 pt-3">
          <label class="block">
            <span class="sr-only">Search skills</span>
            <input
              type="search"
              bind:value={skillQuery}
              placeholder="search skills…"
              aria-label="Search skills"
              class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
            />
          </label>
        </div>
        <ul class="max-h-64 overflow-y-auto px-2 py-2 text-xs">
          {#each filteredSkills as s (s.slug)}
            <li>
              <button
                type="button"
                onclick={() => add('skill', s)}
                class="flex w-full items-center justify-between gap-2 rounded px-2 py-1 text-left font-mono hover:bg-[var(--sp-bg)]"
              >
                <span class="truncate text-[var(--sp-fg)]">{s.slug}</span>
                <Plus size="11" class="shrink-0 text-[var(--sp-muted-fg)]" />
              </button>
            </li>
          {:else}
            <li class="px-2 py-1 text-[var(--sp-muted-fg)]">No matches.</li>
          {/each}
        </ul>
        {#if selectedSkills.length > 0}
          <div class="border-t border-[var(--sp-border)] px-3 py-2">
            <p
              class="mb-1 text-[10px] font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
            >
              Selected
            </p>
            <ul class="space-y-1 text-xs">
              {#each selectedSkills as p (p.slug)}
                <li
                  class="flex items-center justify-between gap-2 rounded bg-[var(--sp-bg)] px-2 py-1"
                >
                  <span class="truncate font-mono text-[var(--sp-fg)]">{p.slug}@{p.version}</span>
                  <button
                    type="button"
                    onclick={() => remove('skill', p.slug)}
                    aria-label={`Remove skill ${p.slug}`}
                    class="rounded p-1 text-[var(--sp-muted-fg)] hover:bg-red-50 hover:text-red-700"
                  >
                    <X size="11" />
                  </button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>

      <!-- Agents column -->
      <div class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]">
        <header
          class="border-b border-[var(--sp-border)] px-3 py-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
        >
          Agents · {selectedAgents.length}
        </header>
        <div class="px-3 pt-3">
          <label class="block">
            <span class="sr-only">Search agents</span>
            <input
              type="search"
              bind:value={agentQuery}
              placeholder="search agents…"
              aria-label="Search agents"
              class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
            />
          </label>
        </div>
        <ul class="max-h-64 overflow-y-auto px-2 py-2 text-xs">
          {#each filteredAgents as a (a.slug)}
            <li>
              <button
                type="button"
                onclick={() => add('agent', a)}
                class="flex w-full items-center justify-between gap-2 rounded px-2 py-1 text-left font-mono hover:bg-[var(--sp-bg)]"
              >
                <span class="truncate text-[var(--sp-fg)]">{a.slug}</span>
                <Plus size="11" class="shrink-0 text-[var(--sp-muted-fg)]" />
              </button>
            </li>
          {:else}
            <li class="px-2 py-1 text-[var(--sp-muted-fg)]">No matches.</li>
          {/each}
        </ul>
        {#if selectedAgents.length > 0}
          <div class="border-t border-[var(--sp-border)] px-3 py-2">
            <p
              class="mb-1 text-[10px] font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
            >
              Selected
            </p>
            <ul class="space-y-1 text-xs">
              {#each selectedAgents as p (p.slug)}
                <li
                  class="flex items-center justify-between gap-2 rounded bg-[var(--sp-bg)] px-2 py-1"
                >
                  <span class="truncate font-mono text-[var(--sp-fg)]">{p.slug}@{p.version}</span>
                  <button
                    type="button"
                    onclick={() => remove('agent', p.slug)}
                    aria-label={`Remove agent ${p.slug}`}
                    class="rounded p-1 text-[var(--sp-muted-fg)] hover:bg-red-50 hover:text-red-700"
                  >
                    <X size="11" />
                  </button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>

      <!-- Commands column -->
      <div class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]">
        <header
          class="border-b border-[var(--sp-border)] px-3 py-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
        >
          Commands · {selectedCommands.length}
        </header>
        <div class="px-3 pt-3">
          <label class="block">
            <span class="sr-only">Search commands</span>
            <input
              type="search"
              bind:value={commandQuery}
              placeholder="search commands…"
              aria-label="Search commands"
              class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
            />
          </label>
        </div>
        <ul class="max-h-64 overflow-y-auto px-2 py-2 text-xs">
          {#each filteredCommands as c (c.slug)}
            <li>
              <button
                type="button"
                onclick={() => add('command', c)}
                class="flex w-full items-center justify-between gap-2 rounded px-2 py-1 text-left font-mono hover:bg-[var(--sp-bg)]"
              >
                <span class="truncate text-[var(--sp-fg)]">{c.slug}</span>
                <Plus size="11" class="shrink-0 text-[var(--sp-muted-fg)]" />
              </button>
            </li>
          {:else}
            <li class="px-2 py-1 text-[var(--sp-muted-fg)]">No matches.</li>
          {/each}
        </ul>
        {#if selectedCommands.length > 0}
          <div class="border-t border-[var(--sp-border)] px-3 py-2">
            <p
              class="mb-1 text-[10px] font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
            >
              Selected
            </p>
            <ul class="space-y-1 text-xs">
              {#each selectedCommands as p (p.slug)}
                <li
                  class="flex items-center justify-between gap-2 rounded bg-[var(--sp-bg)] px-2 py-1"
                >
                  <span class="truncate font-mono text-[var(--sp-fg)]">{p.slug}@{p.version}</span>
                  <button
                    type="button"
                    onclick={() => remove('command', p.slug)}
                    aria-label={`Remove command ${p.slug}`}
                    class="rounded p-1 text-[var(--sp-muted-fg)] hover:bg-red-50 hover:text-red-700"
                  >
                    <X size="11" />
                  </button>
                </li>
              {/each}
            </ul>
          </div>
        {/if}
      </div>
    </div>
  </section>

  <!-- ─── Optional inline JSON blobs ──────────────────────────────────── -->
  <section class="max-w-3xl">
    <h2 class="mb-1 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
      Optional inline blobs
    </h2>
    <p class="mb-3 text-xs text-[var(--sp-muted-fg)]">
      Pasted JSON is parsed on submit. Parse errors surface inline next to the offending section.
    </p>

    <div class="space-y-4">
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">hooks</span>
        <textarea
          name="hooks_json"
          rows="3"
          bind:value={hooksJson}
          placeholder={'e.g. { "PreToolUse": [...] }'}
          class="mt-1 w-full rounded-[var(--sp-radius)] border bg-[var(--sp-bg)] px-3 py-2 font-mono text-xs focus:outline-none {erroredSection ===
          'hooks'
            ? 'border-red-400 focus:border-red-500'
            : 'border-[var(--sp-border)] focus:border-[var(--sp-primary)]'}"
        ></textarea>
      </label>
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">mcpServers</span>
        <textarea
          name="mcp_servers_json"
          rows="3"
          bind:value={mcpServersJson}
          placeholder={'e.g. { "linear": { "command": "npx", "args": [...] } }'}
          class="mt-1 w-full rounded-[var(--sp-radius)] border bg-[var(--sp-bg)] px-3 py-2 font-mono text-xs focus:outline-none {erroredSection ===
          'mcpServers'
            ? 'border-red-400 focus:border-red-500'
            : 'border-[var(--sp-border)] focus:border-[var(--sp-primary)]'}"
        ></textarea>
      </label>
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">lspServers</span>
        <textarea
          name="lsp_servers_json"
          rows="3"
          bind:value={lspServersJson}
          placeholder={'e.g. { "rust-analyzer": { "command": "rust-analyzer" } }'}
          class="mt-1 w-full rounded-[var(--sp-radius)] border bg-[var(--sp-bg)] px-3 py-2 font-mono text-xs focus:outline-none {erroredSection ===
          'lspServers'
            ? 'border-red-400 focus:border-red-500'
            : 'border-[var(--sp-border)] focus:border-[var(--sp-primary)]'}"
        ></textarea>
      </label>
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">monitors (experimental)</span>
        <textarea
          name="monitors_json"
          rows="3"
          bind:value={monitorsJson}
          placeholder="Wrapped under experimental.monitors in the manifest."
          class="mt-1 w-full rounded-[var(--sp-radius)] border bg-[var(--sp-bg)] px-3 py-2 font-mono text-xs focus:outline-none {erroredSection ===
          'monitors'
            ? 'border-red-400 focus:border-red-500'
            : 'border-[var(--sp-border)] focus:border-[var(--sp-primary)]'}"
        ></textarea>
      </label>
    </div>
  </section>

  <div class="flex items-center gap-3 pt-2">
    <button
      type="submit"
      disabled={overCap || totalSelected === 0}
      class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium disabled:cursor-not-allowed disabled:opacity-50"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Publish plugin
    </button>
    <a href="/admin/plugins" class="text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]">
      Cancel
    </a>
  </div>
</form>
