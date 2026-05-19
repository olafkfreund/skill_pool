<script lang="ts">
  import {
    AlertTriangle,
    Archive,
    ArrowLeft,
    ArrowUpRight,
    Download,
    FileCode,
    GitMerge,
    Link as LinkIcon,
    Activity,
  } from '@lucide/svelte';
  import MonacoViewer from '$lib/MonacoViewer.svelte';

  let { data, form } = $props();

  const d = $derived(data.detail);
  // Brand name used in the og:title. Falls back to the tenant slug when
  // the tenant hasn't set a brand_name yet — mirrors the server's own
  // default-theme behaviour.
  const brandName = $derived(data.theme?.brandName ?? data.tenant?.slug ?? 'skill-pool');

  function fmtDate(iso: string | null): string {
    if (!iso) return '—';
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

  function pct(v: number | null): string {
    if (v == null) return '—';
    return `${(v * 100).toFixed(0)}%`;
  }
</script>

<!--
  Open Graph + Twitter Card metadata (#9). The image is served by the
  registry server at `/v1/og`, which renders an SVG with the tenant's
  brand colours + skill name + version. Crawlers (Slack, Discord,
  Twitter, LinkedIn) cache aggressively — see
  docs/enterprise/og-images.md for the platform-by-platform support
  matrix and cache caveats.
-->
<svelte:head>
  <title>{d.slug} · {brandName}</title>
  <meta name="description" content={d.description} />
  <meta property="og:title" content={`${d.slug} · ${brandName}`} />
  <meta property="og:description" content={d.description} />
  <meta property="og:image" content={data.ogImageUrl} />
  <meta property="og:url" content={data.pageUrl} />
  <meta property="og:type" content="article" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta name="twitter:title" content={`${d.slug} · ${brandName}`} />
  <meta name="twitter:description" content={d.description} />
  <meta name="twitter:image" content={data.ogImageUrl} />
</svelte:head>

<a
  href="/"
  class="mb-6 inline-flex items-center gap-1 text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
>
  <ArrowLeft size="14" /> Catalog
</a>

<header class="mb-6">
  <div class="flex flex-wrap items-baseline gap-3">
    <h1 class="text-3xl font-semibold">{d.slug}</h1>
    <span class="text-sm text-[var(--sp-muted-fg)]">v{d.version}</span>
    <span
      class="rounded-full px-2 py-0.5 text-xs"
      style="background: var(--sp-muted); color: var(--sp-muted-fg);"
    >
      {d.status}
    </span>
  </div>
  <p class="mt-3 max-w-prose text-[var(--sp-fg)]">{d.description}</p>
  {#if d.tags.length > 0}
    <div class="mt-4 flex flex-wrap gap-1">
      {#each d.tags as tag (tag)}
        <span
          class="rounded-full px-2 py-0.5 text-xs"
          style="background: var(--sp-muted); color: var(--sp-fg); border: 1px solid var(--sp-border);"
          >{tag}</span
        >
      {/each}
    </div>
  {/if}
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{/if}

<!-- Usage stats + install row -->
<div class="mb-8 grid gap-4 sm:grid-cols-3">
  <div
    class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
  >
    <div class="flex items-center gap-1.5 text-xs tracking-wider text-[var(--sp-muted-fg)] uppercase">
      <Activity size="11" /> Use count
    </div>
    <div class="mt-1 font-mono text-2xl font-semibold text-[var(--sp-fg)]">{d.use_count}</div>
    <div class="mt-1 text-xs text-[var(--sp-muted-fg)]">
      Last: {fmtDate(d.last_used_at)}
    </div>
  </div>
  <div
    class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
  >
    <div class="text-xs tracking-wider text-[var(--sp-muted-fg)] uppercase">Created</div>
    <div class="mt-1 text-sm text-[var(--sp-fg)]">{fmtDate(d.created_at)}</div>
  </div>
  <div
    class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
  >
    <div class="text-xs tracking-wider text-[var(--sp-muted-fg)] uppercase">Install</div>
    <pre
      class="mt-1 overflow-x-auto text-xs"><code>skill-pool add {d.slug}</code></pre>
  </div>
</div>

<!-- Pending merge proposals: amber callout -->
{#if d.merge_proposals.length > 0}
  <section class="mb-8">
    <h2
      class="mb-3 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
    >
      <GitMerge size="13" /> Pending merge proposals · {d.merge_proposals.length}
    </h2>
    <ul class="space-y-2">
      {#each d.merge_proposals as p (p.draft_id)}
        <li>
          <a
            href={`/drafts/${encodeURIComponent(p.draft_id)}/edit`}
            class="flex items-center justify-between gap-3 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-900 hover:border-amber-400"
          >
            <span>
              <code class="font-mono">{p.draft_slug}</code> looks similar
              ({pct(p.similarity)} match)
            </span>
            <ArrowUpRight size="14" />
          </a>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<!-- Dependencies (forward) -->
{#if d.requires.length > 0}
  <section class="mb-8">
    <h2
      class="mb-3 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
    >
      <LinkIcon size="13" /> Requires · {d.requires.length}
    </h2>
    <ul class="space-y-1.5">
      {#each d.requires as req (req.slug)}
        <li
          class="flex items-center justify-between rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-2 text-sm"
        >
          <span>
            {#if req.version}
              <a
                href={`/skills/${encodeURIComponent(req.slug)}`}
                class="font-medium text-[var(--sp-fg)] hover:underline">{req.slug}</a
              >
              <span class="ml-1 text-xs text-[var(--sp-muted-fg)]">v{req.version}</span>
            {:else}
              <span class="font-medium text-[var(--sp-fg)]">{req.slug}</span>
              <span
                class="ml-1 inline-flex items-center gap-1 rounded-full bg-amber-100 px-1.5 py-0.5 text-[10px] text-amber-800"
                title="No published version yet — forward reference"
              >
                <AlertTriangle size="10" /> unpublished
              </span>
            {/if}
          </span>
          <span class="font-mono text-xs text-[var(--sp-muted-fg)]">
            requires {req.version_range}
          </span>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<!-- Required-by (reverse) -->
{#if d.required_by.length > 0}
  <section class="mb-8">
    <h2
      class="mb-3 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
    >
      <LinkIcon size="13" class="rotate-180" /> Required by · {d.required_by.length}
    </h2>
    <ul class="space-y-1.5">
      {#each d.required_by as r (r.slug)}
        <li
          class="flex items-center justify-between rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] px-3 py-2 text-sm"
        >
          <span>
            <a
              href={`/skills/${encodeURIComponent(r.slug)}`}
              class="font-medium text-[var(--sp-fg)] hover:underline">{r.slug}</a
            >
            <span class="ml-1 text-xs text-[var(--sp-muted-fg)]">v{r.version}</span>
          </span>
          <span class="font-mono text-xs text-[var(--sp-muted-fg)]">
            range {r.version_range}
          </span>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<!-- When to use -->
{#if d.when_to_use}
  <section class="mb-8">
    <h2 class="mb-2 text-sm font-semibold tracking-wide text-[var(--sp-muted-fg)] uppercase">
      When to use
    </h2>
    <p class="text-[var(--sp-fg)]">{d.when_to_use}</p>
  </section>
{/if}

<!-- SKILL.md body -->
<section class="mb-8">
  <h2
    class="mb-2 flex items-center gap-2 text-sm font-semibold tracking-wide text-[var(--sp-muted-fg)] uppercase"
  >
    <FileCode size="14" /> SKILL.md
  </h2>
  {#if data.body}
    <MonacoViewer value={data.body} language="markdown" readOnly height="32rem" />
  {:else}
    <p class="text-sm text-[var(--sp-muted-fg)]">
      (couldn't fetch the SKILL.md body; the download link below still works)
    </p>
  {/if}
</section>

<!-- Actions row: download + admin-only archive -->
<section class="flex flex-wrap items-center gap-2">
  <a
    href={`/skills/${encodeURIComponent(d.slug)}/bundle.tar.gz`}
    data-sveltekit-reload
    class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-2 text-sm hover:border-[var(--sp-primary)]"
  >
    <Download size="14" />
    Download bundle.tar.gz
  </a>
  <form method="POST" action="?/archive">
    <button
      type="submit"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] border border-red-200 px-3 py-2 text-sm text-red-700 hover:bg-red-50"
      title="Move this skill to the graveyard. Admins only."
    >
      <Archive size="14" /> Archive
    </button>
  </form>
</section>
