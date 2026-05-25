<script lang="ts">
  import { AlertTriangle, BarChart3, Download, Eye, Trophy } from '@lucide/svelte';
  import type { TimelineBucket } from '$lib/server/api';

  let { data } = $props();

  // Bar chart dimensions. Hand-rolled SVG to avoid a chart-library dep.
  const W = 720;
  const H = 180;
  const PAD = { top: 12, right: 12, bottom: 22, left: 36 };

  const innerW = $derived(W - PAD.left - PAD.right);
  const innerH = $derived(H - PAD.top - PAD.bottom);

  const maxTotal = $derived(
    Math.max(1, ...data.timeline.map((b: TimelineBucket) => b.downloads + b.views)),
  );

  const barWidth = $derived(data.timeline.length > 0 ? innerW / data.timeline.length : 0);

  function fmtDay(iso: string): string {
    try {
      return new Date(iso).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
    } catch {
      return iso;
    }
  }

  function bucketTotal(b: TimelineBucket): number {
    return b.downloads + b.views;
  }
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <BarChart3 size="22" /> Usage
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Catalog activity over the last <strong>{data.days}</strong> days. Every bundle download is a
    <em>download</em>; every SKILL.md fetch (e.g. the inbox preview or the MCP `get_skill` tool) is
    a <em>view</em>. Tune the window with
    <code class="rounded bg-[var(--sp-muted)] px-1">?days=7</code> or
    <code class="rounded bg-[var(--sp-muted)] px-1">?days=90</code>.
  </p>
</header>

<form class="mb-6 flex items-end gap-3 text-sm" data-sveltekit-reload>
  <label class="block">
    <span class="block text-xs text-[var(--sp-muted-fg)]">Window (days)</span>
    <input
      type="number"
      name="days"
      min="1"
      max="365"
      value={data.days}
      class="mt-0.5 w-24 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 font-mono text-xs"
    />
  </label>
  <button
    type="submit"
    class="rounded-[var(--sp-radius)] px-4 py-1.5 text-sm font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    Reload
  </button>
</form>

{#if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

<section class="mb-10">
  <h2 class="mb-3 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    Daily activity
  </h2>
  <div
    class="overflow-x-auto rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
  >
    <svg viewBox="0 0 {W} {H}" class="block h-auto w-full" role="img" aria-label="Daily activity">
      <!-- Horizontal axis baseline -->
      <line
        x1={PAD.left}
        y1={PAD.top + innerH}
        x2={W - PAD.right}
        y2={PAD.top + innerH}
        stroke="var(--sp-border)"
        stroke-width="1"
      />
      <!-- Y-axis ticks (just min/max) -->
      <text
        x={PAD.left - 6}
        y={PAD.top}
        text-anchor="end"
        dominant-baseline="middle"
        font-size="10"
        fill="var(--sp-muted-fg)">{maxTotal}</text
      >
      <text
        x={PAD.left - 6}
        y={PAD.top + innerH}
        text-anchor="end"
        dominant-baseline="middle"
        font-size="10"
        fill="var(--sp-muted-fg)">0</text
      >

      {#each data.timeline as b, i (b.day)}
        {@const total = bucketTotal(b)}
        {@const downloadsH = (b.downloads / maxTotal) * innerH}
        {@const viewsH = (b.views / maxTotal) * innerH}
        {@const x = PAD.left + i * barWidth + 1}
        {@const w = Math.max(0, barWidth - 2)}
        <g>
          <!-- Stacked: downloads on bottom (primary), views on top (accent) -->
          <rect
            {x}
            y={PAD.top + innerH - downloadsH}
            width={w}
            height={downloadsH}
            fill="var(--sp-primary)"
          >
            <title>{fmtDay(b.day)}: {b.downloads} downloads, {b.views} views</title>
          </rect>
          <rect
            {x}
            y={PAD.top + innerH - downloadsH - viewsH}
            width={w}
            height={viewsH}
            fill="var(--sp-accent)"
            opacity="0.85"
          >
            <title>{fmtDay(b.day)}: {b.downloads} downloads, {b.views} views</title>
          </rect>
          {#if total === 0 && data.timeline.length <= 14}
            <!-- Faint marker for empty days when the window is short. -->
            <rect {x} y={PAD.top + innerH - 2} width={w} height="2" fill="var(--sp-border)" />
          {/if}
        </g>
        <!-- X-axis labels: show ~every 5th day to avoid overlap. -->
        {#if i % Math.max(1, Math.floor(data.timeline.length / 8)) === 0 || i === data.timeline.length - 1}
          <text
            x={x + w / 2}
            y={H - 6}
            text-anchor="middle"
            font-size="10"
            fill="var(--sp-muted-fg)">{fmtDay(b.day)}</text
          >
        {/if}
      {/each}
    </svg>
    <div class="mt-3 flex flex-wrap items-center gap-4 text-xs text-[var(--sp-muted-fg)]">
      <span class="inline-flex items-center gap-1.5">
        <span class="inline-block h-3 w-3 rounded-sm" style="background: var(--sp-primary);"></span>
        <Download size="11" /> Downloads
      </span>
      <span class="inline-flex items-center gap-1.5">
        <span
          class="inline-block h-3 w-3 rounded-sm opacity-85"
          style="background: var(--sp-accent);"
        ></span>
        <Eye size="11" /> Views
      </span>
    </div>
  </div>
</section>

<section>
  <h2
    class="mb-3 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
  >
    <Trophy size="13" /> Top skills · last {data.days} days
  </h2>
  {#if data.top.length === 0}
    <div
      class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
    >
      No activity in this window yet.
    </div>
  {:else}
    <div
      class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]"
    >
      <table class="w-full text-sm">
        <thead
          class="bg-[var(--sp-bg)] text-left text-xs tracking-wide text-[var(--sp-muted-fg)] uppercase"
        >
          <tr>
            <th class="px-4 py-3">#</th>
            <th class="px-4 py-3">Skill</th>
            <th class="px-4 py-3 text-right">Downloads</th>
            <th class="px-4 py-3 text-right">Views</th>
            <th class="px-4 py-3 text-right">Total</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-[var(--sp-border)]">
          {#each data.top as row, i (row.slug)}
            <tr>
              <td class="px-4 py-3 font-mono text-xs text-[var(--sp-muted-fg)]">{i + 1}</td>
              <td class="px-4 py-3">
                <a
                  href={`/skills/${encodeURIComponent(row.slug)}`}
                  class="font-medium text-[var(--sp-fg)] hover:underline">{row.slug}</a
                >
              </td>
              <td class="px-4 py-3 text-right font-mono text-xs">{row.downloads}</td>
              <td class="px-4 py-3 text-right font-mono text-xs">{row.views}</td>
              <td class="px-4 py-3 text-right font-mono text-xs font-semibold">{row.total}</td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>
