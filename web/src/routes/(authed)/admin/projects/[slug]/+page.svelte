<script lang="ts">
  import { untrack } from 'svelte';
  import {
    AlertTriangle,
    CheckCircle2,
    FolderGit2,
    Plus,
    Save,
    Tag,
    Trash2,
  } from '@lucide/svelte';
  import type { ProjectItem } from '$lib/server/api';

  let { data, form } = $props();

  const project = $derived(data.project);

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

  // Convenience: which action just ran?
  const lastAction = $derived(form?.action as string | undefined);
  const metaSaved = $derived(lastAction === 'meta' && form?.saved);
  const tagsSaved = $derived(lastAction === 'tags' && form?.saved);
  const itemAdded = $derived(lastAction === 'addItem' && form?.added);
  const itemRemoved = $derived(lastAction === 'removeItem' && form?.removed);
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
