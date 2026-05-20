<script lang="ts">
  import { untrack } from 'svelte';
  import {
    AlertTriangle,
    CheckCircle2,
    FileText,
    FolderGit2,
    Plus,
    RefreshCw,
    Save,
    Tag,
    Trash2,
  } from '@lucide/svelte';
  import type { ProjectItem, ProjectPlan, ProjectPlanVersion } from '$lib/server/api';

  let { data, form } = $props();

  const project = $derived(data.project);
  const plan = $derived(data.plan as ProjectPlan | null);
  const planVersions = $derived((data.planVersions ?? []) as ProjectPlanVersion[]);
  const isCurator = $derived(
    data.userRole === 'curator' || data.userRole === 'admin',
  );

  // Separate items by kind for the three sub-tables.
  const skills = $derived(project.items.filter((it: ProjectItem) => it.kind === 'skill'));
  const agents = $derived(project.items.filter((it: ProjectItem) => it.kind === 'agent'));
  const commands = $derived(project.items.filter((it: ProjectItem) => it.kind === 'command'));

  // Chip preview for stack_tags input — untrack so Svelte doesn't warn about
  // capturing only the initial value of the derived `project`. This is
  // intentional: we seed the field once from server data, then let the user
  // freely edit it.
  let tagsInput = $state(untrack(() => project.stack_tags.join(', ')));
  const tagChips = $derived(
    tagsInput
      .split(',')
      .map((t) => t.trim())
      .filter(Boolean),
  );

  // Auto-refresh form state — seeded from the project's
  // `plan_auto_refresh_interval_secs` (null = explicit-only).
  // We default to 3600 s (1 h) when the toggle is first enabled.
  const existingInterval = $derived(project.plan_auto_refresh_interval_secs);
  let autoRefreshEnabled = $state(untrack(() => !!existingInterval));
  let autoRefreshInterval = $state(
    untrack(() => (existingInterval && existingInterval >= 300 ? existingInterval : 3600)),
  );

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

  /** Relative-time helper: "3 hours ago", "2 days ago", etc. */
  function relTime(iso: string): string {
    try {
      const diff = Date.now() - new Date(iso).getTime();
      const mins = Math.floor(diff / 60_000);
      if (mins < 2) return 'just now';
      if (mins < 60) return `${mins} minutes ago`;
      const hrs = Math.floor(mins / 60);
      if (hrs < 24) return `${hrs} hour${hrs === 1 ? '' : 's'} ago`;
      const days = Math.floor(hrs / 24);
      return `${days} day${days === 1 ? '' : 's'} ago`;
    } catch {
      return iso;
    }
  }

  /** Truncate a URL to at most `max` chars, appending "…" if needed. */
  function truncUrl(url: string | null | undefined, max = 60): string {
    if (!url) return '';
    return url.length > max ? url.slice(0, max) + '…' : url;
  }

  // Convenience: which action just ran?
  const lastAction = $derived(form?.action as string | undefined);
  const metaSaved = $derived(lastAction === 'meta' && form?.saved);
  const tagsSaved = $derived(lastAction === 'tags' && form?.saved);
  const itemAdded = $derived(lastAction === 'addItem' && form?.added);
  const itemRemoved = $derived(lastAction === 'removeItem' && form?.removed);
  const planVersionActivated = $derived(lastAction === 'activatePlanVersion' && form?.activated);
  const autoRefreshSaved = $derived(lastAction === 'setAutoRefresh' && form?.saved);
  const hasError = $derived(!!form?.error);
</script>

<header class="mb-6">
  <nav class="mb-2 text-xs text-[var(--sp-muted-fg)]">
    <a href="/admin/projects" class="hover:underline">Projects</a>
    <span class="mx-1">/</span>
    <span class="font-mono">{project.slug}</span>
  </nav>
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <FolderGit2 size="22" />
    {project.name}
  </h1>
  <p class="mt-1 font-mono text-xs text-[var(--sp-muted-fg)]">{project.slug}</p>
  <p class="mt-1 text-xs text-[var(--sp-muted-fg)]">
    Last updated {fmtDate(project.updated_at)}
  </p>
</header>

<!-- Global toast row -->
{#if hasError}
  <div
    class="mb-6 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form?.error}</span>
  </div>
{:else if metaSaved}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Metadata saved.
  </div>
{:else if tagsSaved}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Stack tags saved.
  </div>
{:else if itemAdded}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Added
    <code class="rounded bg-emerald-100 px-1">{form?.skill_slug}</code> ({form?.kind}).
  </div>
{:else if itemRemoved}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Removed
    <code class="rounded bg-emerald-100 px-1">{form?.skill_slug}</code> ({form?.kind}).
  </div>
{:else if planVersionActivated}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Plan version {form?.version} activated.
  </div>
{:else if autoRefreshSaved}
  <div
    class="mb-6 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" />
    {form?.interval_secs ? `Auto-refresh set to every ${form.interval_secs} seconds.` : 'Auto-refresh disabled.'}
  </div>
{/if}

<!-- ─── Metadata section ─────────────────────────────────────────────────── -->
<section class="mb-8 max-w-xl">
  <h2 class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Metadata
  </h2>
  <form method="POST" action="?/updateMeta" class="space-y-4">
    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">
        Name <span class="text-red-500">*</span>
      </span>
      <input
        type="text"
        name="name"
        required
        value={project.name}
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
      >{project.description ?? ''}</textarea>
    </label>

    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">Git remote</span>
      <input
        type="url"
        name="git_remote"
        value={project.git_remote ?? ''}
        placeholder="https://github.com/acme/billing-service"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
        Used for auto-discovery when developers run
        <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool bootstrap</code> in the repo.
      </span>
    </label>

    <button
      type="submit"
      class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      <Save size="14" /> Save metadata
    </button>
  </form>
</section>

<!-- ─── Stack tags section ───────────────────────────────────────────────── -->
<section class="mb-8 max-w-xl">
  <h2 class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Stack tags
  </h2>
  <p class="mb-3 text-sm text-[var(--sp-muted-fg)]">
    Comma-separated tags that echo the detected stack (e.g.
    <code class="rounded bg-[var(--sp-muted)] px-1">rust, axum, postgres</code>). Used to
    back-fill stack-mapping slots when the project doesn't fully cover them.
  </p>
  <form method="POST" action="?/setTags" class="space-y-3">
    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">Tags</span>
      <input
        type="text"
        name="stack_tags"
        bind:value={tagsInput}
        placeholder="rust, axum, postgres"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
    </label>

    {#if tagChips.length > 0}
      <div class="flex flex-wrap gap-1.5">
        {#each tagChips as chip (chip)}
          <span
            class="inline-flex items-center gap-1 rounded-full border border-[var(--sp-border)] bg-[var(--sp-muted)] px-2 py-0.5 font-mono text-xs text-[var(--sp-fg)]"
          >
            <Tag size="10" />
            {chip}
          </span>
        {/each}
      </div>
    {/if}

    <button
      type="submit"
      class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm font-medium hover:border-[var(--sp-primary)]"
    >
      <Save size="14" /> Save tags
    </button>
  </form>
</section>

<!-- ─── Plan section ────────────────────────────────────────────────────── -->
<section class="mb-8 max-w-3xl">
  <h2
    class="mb-1 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
  >
    <FileText size="13" /> Plan
  </h2>
  <p class="mb-4 text-xs text-[var(--sp-muted-fg)]">
    Project plans are authored externally (Confluence, Notion, GitHub, local file) and imported
    via the CLI. See the
    <a href="/docs/wiki/Projects.md" class="underline hover:text-[var(--sp-fg)]"
      >Projects docs</a
    > for the import flow.
  </p>

  {#if plan}
    <!-- ── Active version card ── -->
    <div
      class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)]"
    >
      <!-- Stale-fetch warning chip -->
      {#if plan.fetch_error}
        <div
          class="flex items-start gap-2 border-b border-amber-200 bg-amber-50 px-4 py-2 text-xs text-amber-800"
        >
          <AlertTriangle size="13" class="mt-0.5 shrink-0" />
          <span>
            Last refresh failed: {plan.fetch_error}
            {#if plan.fetch_error_at}
              (at {fmtDate(plan.fetch_error_at)})
            {/if}
          </span>
        </div>
      {/if}

      <!-- Top row: version chip + status pill + import info -->
      <div class="flex flex-wrap items-center gap-2 border-b border-[var(--sp-border)] px-4 py-3">
        <span
          class="rounded-full bg-[var(--sp-muted)] px-2 py-0.5 font-mono text-xs font-semibold text-[var(--sp-fg)]"
        >
          v{plan.version}
        </span>
        <span
          class="rounded-full border border-[var(--sp-border)] px-2 py-0.5 text-xs text-[var(--sp-muted-fg)]"
        >
          {plan.status}
        </span>
        <span class="text-xs text-[var(--sp-muted-fg)]">
          imported {relTime(plan.imported_at)}
          {#if plan.imported_by_email}
            by {plan.imported_by_email}
          {/if}
        </span>
      </div>

      <!-- Source row -->
      <div class="flex items-center gap-1 border-b border-[var(--sp-border)] px-4 py-2 text-xs text-[var(--sp-muted-fg)]">
        <span>from: <span class="font-mono">{plan.source_type}</span></span>
        {#if plan.source_url}
          <span>·</span>
          <a
            href={plan.source_url}
            target="_blank"
            rel="noopener noreferrer"
            class="underline hover:text-[var(--sp-fg)]"
            title={plan.source_url}
          >
            {truncUrl(plan.source_url)} ↗
          </a>
        {/if}
      </div>

      <!-- Body: raw markdown in <pre> — no heavyweight renderer for v1 -->
      <div class="max-h-96 overflow-y-auto px-4 py-3">
        <pre class="whitespace-pre-wrap text-sm text-[var(--sp-fg)]">{plan.body_md}</pre>
      </div>
    </div>

    <!-- ── Version history (collapsed by default) ── -->
    {#if planVersions.length > 0}
      <details class="mt-4">
        <summary
          class="cursor-pointer select-none rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-2 text-xs font-medium text-[var(--sp-fg)] hover:border-[var(--sp-primary)]"
        >
          Version history ({planVersions.length} version{planVersions.length === 1 ? '' : 's'})
        </summary>
        <div class="mt-2 overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]">
          <table class="w-full text-xs">
            <thead
              class="bg-[var(--sp-muted)] text-left tracking-wide text-[var(--sp-muted-fg)] uppercase"
            >
              <tr>
                <th class="px-3 py-2 font-medium">Version</th>
                <th class="px-3 py-2 font-medium">Status</th>
                <th class="px-3 py-2 font-medium">Imported</th>
                <th class="px-3 py-2 font-medium">By</th>
                <th class="px-3 py-2 font-medium">Source</th>
                <th class="px-3 py-2 font-medium text-right">Action</th>
              </tr>
            </thead>
            <tbody class="divide-y divide-[var(--sp-border)] bg-[var(--sp-bg)]">
              {#each planVersions as v (v.version)}
                <tr>
                  <td class="px-3 py-2 font-mono">v{v.version}</td>
                  <td class="px-3 py-2">
                    <span
                      class="rounded-full border border-[var(--sp-border)] bg-[var(--sp-muted)] px-1.5 py-0.5 text-[10px] text-[var(--sp-muted-fg)]"
                    >
                      {v.status}
                    </span>
                  </td>
                  <td class="px-3 py-2 text-[var(--sp-muted-fg)]">
                    {fmtDate(v.imported_at)}
                  </td>
                  <td class="px-3 py-2 text-[var(--sp-muted-fg)]">
                    {v.imported_by_email ?? '—'}
                  </td>
                  <td class="px-3 py-2 font-mono text-[var(--sp-muted-fg)]">
                    {#if v.source_url}
                      <a
                        href={v.source_url}
                        target="_blank"
                        rel="noopener noreferrer"
                        class="underline hover:text-[var(--sp-fg)]"
                        title={v.source_url}
                      >
                        {truncUrl(v.source_url, 30)} ↗
                      </a>
                    {:else}
                      {v.source_type}
                    {/if}
                  </td>
                  <td class="px-3 py-2 text-right">
                    {#if v.status !== 'active'}
                      <form method="POST" action="?/activatePlanVersion" class="inline-block">
                        <input type="hidden" name="version" value={v.version} />
                        <button
                          type="submit"
                          class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-2 py-1 text-[10px] font-medium hover:border-[var(--sp-primary)]"
                        >
                          Activate
                        </button>
                      </form>
                    {:else}
                      <span class="text-[10px] text-[var(--sp-muted-fg)]">active</span>
                    {/if}
                  </td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      </details>
    {/if}

    <!-- ── Auto-refresh toggle (curator / admin only) ── -->
    {#if isCurator}
      <div class="mt-4 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-4 py-3">
        <form method="POST" action="?/setAutoRefresh" class="space-y-3">
          <div class="flex items-center gap-3">
            <label class="flex cursor-pointer items-center gap-2 text-sm font-medium text-[var(--sp-fg)]">
              <input
                type="checkbox"
                name="auto_refresh_enabled"
                bind:checked={autoRefreshEnabled}
                class="h-4 w-4 rounded border-[var(--sp-border)]"
              />
              Auto-refresh from source
            </label>
          </div>

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
                placeholder="3600"
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
            <RefreshCw size="11" /> Save
          </button>
        </form>
      </div>
    {/if}
  {:else}
    <!-- ── Empty state: no plan imported ── -->
    <div
      class="rounded-[var(--sp-radius)] border-2 border-dashed border-[var(--sp-border)] bg-[var(--sp-muted)] px-6 py-8 text-center text-sm text-[var(--sp-muted-fg)]"
    >
      <FileText size="24" class="mx-auto mb-2 opacity-40" />
      <p class="font-medium text-[var(--sp-fg)]">No plan imported yet.</p>
      <p class="mt-1">
        Use
        <code class="rounded bg-[var(--sp-bg)] px-1 py-0.5 font-mono text-xs">
          skill-pool plan import {project.slug} --file ./plan.md
        </code>
        or
        <code class="rounded bg-[var(--sp-bg)] px-1 py-0.5 font-mono text-xs">--url https://...</code>
        to import a plan.
      </p>
    </div>
  {/if}
</section>

<!-- ─── Items section ───────────────────────────────────────────────────── -->
<section class="mb-8">
  <h2 class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Curated items
  </h2>
  <p class="mb-5 text-sm text-[var(--sp-muted-fg)]">
    Skills, agents, and commands in this project are installed by
    <code class="rounded bg-[var(--sp-muted)] px-1">skill-pool bootstrap</code> before any
    stack-mapping backfill. Forward references (slugs that don't exist yet) are allowed.
  </p>

  <div class="space-y-6">
    <!-- Skills sub-table -->
    <div>
      <h3 class="mb-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
        Skills
      </h3>
      <div class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]">
        <table class="w-full text-sm">
          <thead
            class="bg-[var(--sp-muted)] text-left text-xs tracking-wide text-[var(--sp-muted-fg)] uppercase"
          >
            <tr>
              <th class="px-4 py-2">Slug</th>
              <th class="px-4 py-2 text-right">Action</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-[var(--sp-border)] bg-[var(--sp-bg)]">
            {#each skills as item (item.skill_slug)}
              <tr>
                <td class="px-4 py-2.5 font-mono text-xs text-[var(--sp-fg)]">
                  <a
                    href={`/skills/${encodeURIComponent(item.skill_slug)}`}
                    class="hover:underline"
                  >
                    {item.skill_slug}
                  </a>
                </td>
                <td class="px-4 py-2.5 text-right">
                  <form method="POST" action="?/removeItem" class="inline-block">
                    <input type="hidden" name="skill_slug" value={item.skill_slug} />
                    <input type="hidden" name="kind" value="skill" />
                    <button
                      type="submit"
                      title={`Remove skill ${item.skill_slug}`}
                      class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                    >
                      <Trash2 size="11" /> Remove
                    </button>
                  </form>
                </td>
              </tr>
            {/each}
            <tr class="bg-[var(--sp-muted)]">
              <td colspan="2" class="px-4 py-2.5">
                <form method="POST" action="?/addItem" class="flex items-center gap-2">
                  <input type="hidden" name="kind" value="skill" />
                  <input
                    type="text"
                    name="skill_slug"
                    placeholder="skill-slug"
                    class="w-56 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
                  />
                  <button
                    type="submit"
                    class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] px-3 py-1 text-xs font-medium"
                    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                  >
                    <Plus size="11" /> Add
                  </button>
                </form>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- Agents sub-table -->
    <div>
      <h3 class="mb-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
        Agents
      </h3>
      <div class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]">
        <table class="w-full text-sm">
          <thead
            class="bg-[var(--sp-muted)] text-left text-xs tracking-wide text-[var(--sp-muted-fg)] uppercase"
          >
            <tr>
              <th class="px-4 py-2">Slug</th>
              <th class="px-4 py-2 text-right">Action</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-[var(--sp-border)] bg-[var(--sp-bg)]">
            {#each agents as item (item.skill_slug)}
              <tr>
                <td class="px-4 py-2.5 font-mono text-xs text-[var(--sp-fg)]">
                  <a
                    href={`/skills/${encodeURIComponent(item.skill_slug)}`}
                    class="hover:underline"
                  >
                    {item.skill_slug}
                  </a>
                </td>
                <td class="px-4 py-2.5 text-right">
                  <form method="POST" action="?/removeItem" class="inline-block">
                    <input type="hidden" name="skill_slug" value={item.skill_slug} />
                    <input type="hidden" name="kind" value="agent" />
                    <button
                      type="submit"
                      title={`Remove agent ${item.skill_slug}`}
                      class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                    >
                      <Trash2 size="11" /> Remove
                    </button>
                  </form>
                </td>
              </tr>
            {/each}
            <tr class="bg-[var(--sp-muted)]">
              <td colspan="2" class="px-4 py-2.5">
                <form method="POST" action="?/addItem" class="flex items-center gap-2">
                  <input type="hidden" name="kind" value="agent" />
                  <input
                    type="text"
                    name="skill_slug"
                    placeholder="agent-slug"
                    class="w-56 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
                  />
                  <button
                    type="submit"
                    class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] px-3 py-1 text-xs font-medium"
                    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                  >
                    <Plus size="11" /> Add
                  </button>
                </form>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <!-- Commands sub-table -->
    <div>
      <h3 class="mb-2 text-xs font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
        Commands
      </h3>
      <div class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)]">
        <table class="w-full text-sm">
          <thead
            class="bg-[var(--sp-muted)] text-left text-xs tracking-wide text-[var(--sp-muted-fg)] uppercase"
          >
            <tr>
              <th class="px-4 py-2">Slug</th>
              <th class="px-4 py-2 text-right">Action</th>
            </tr>
          </thead>
          <tbody class="divide-y divide-[var(--sp-border)] bg-[var(--sp-bg)]">
            {#each commands as item (item.skill_slug)}
              <tr>
                <td class="px-4 py-2.5 font-mono text-xs text-[var(--sp-fg)]">
                  <a
                    href={`/skills/${encodeURIComponent(item.skill_slug)}`}
                    class="hover:underline"
                  >
                    {item.skill_slug}
                  </a>
                </td>
                <td class="px-4 py-2.5 text-right">
                  <form method="POST" action="?/removeItem" class="inline-block">
                    <input type="hidden" name="skill_slug" value={item.skill_slug} />
                    <input type="hidden" name="kind" value="command" />
                    <button
                      type="submit"
                      title={`Remove command ${item.skill_slug}`}
                      class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                    >
                      <Trash2 size="11" /> Remove
                    </button>
                  </form>
                </td>
              </tr>
            {/each}
            <tr class="bg-[var(--sp-muted)]">
              <td colspan="2" class="px-4 py-2.5">
                <form method="POST" action="?/addItem" class="flex items-center gap-2">
                  <input type="hidden" name="kind" value="command" />
                  <input
                    type="text"
                    name="skill_slug"
                    placeholder="command-slug"
                    class="w-56 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
                  />
                  <button
                    type="submit"
                    class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] px-3 py-1 text-xs font-medium"
                    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                  >
                    <Plus size="11" /> Add
                  </button>
                </form>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>
  </div>
</section>

<!-- ─── Danger zone ──────────────────────────────────────────────────────── -->
<section class="max-w-xl">
  <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Danger zone
  </h2>
  <form
    method="POST"
    action="?/deleteProject"
    onsubmit={(e) => {
      if (
        !confirm(
          `Delete project "${project.slug}"? All curated items will be lost. This cannot be undone.`,
        )
      ) {
        e.preventDefault();
      }
    }}
    class="flex items-center justify-between rounded-[var(--sp-radius)] border border-red-200 bg-red-50 p-3 text-sm"
  >
    <span class="text-red-800">
      Permanently delete this project and all its curated items.
    </span>
    <button
      type="submit"
      class="ml-3 inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-300 bg-white px-3 py-1.5 text-xs font-medium text-red-700 hover:bg-red-100"
    >
      <Trash2 size="12" /> Delete project
    </button>
  </form>
</section>
