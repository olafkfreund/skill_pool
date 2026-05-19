<script lang="ts">
  import {
    AlertCircle,
    AlertTriangle,
    CheckCircle2,
    Clock,
    Copy,
    Globe,
    RefreshCw,
    ShieldCheck,
    Trash2,
  } from '@lucide/svelte';
  import type { CustomDomain } from '$lib/server/api';

  let { data, form } = $props();

  // After the `add` action the page reloads via SvelteKit's enhance default,
  // so `data.domains` is fresh — we don't need to splice optimistically.
  // The toast banner is the only thing reading from `form` here.

  function fmtDate(iso: string | null | undefined): string {
    if (!iso) return '—';
    try {
      const d = new Date(iso);
      return d.toLocaleString(undefined, {
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

  /**
   * Visual treatment for the row's status pill. `active` and `verified` both
   * mean "in the routing cache, certs can be issued"; the distinction is
   * operational (operator has confirmed the proxy is wired up). Group them
   * visually so admins aren't asked to interpret the difference at a glance.
   */
  function statusBadgeClass(status: string): string {
    switch (status) {
      case 'active':
        return 'bg-emerald-100 text-emerald-800';
      case 'verified':
        return 'bg-sky-100 text-sky-800';
      case 'pending':
        return 'bg-amber-100 text-amber-800';
      case 'failed':
        return 'bg-red-100 text-red-800';
      default:
        return 'bg-slate-100 text-slate-700';
    }
  }

  /** Friendly label for the status. The wire values are the source of truth. */
  function statusLabel(status: string): string {
    switch (status) {
      case 'active':
        return 'active';
      case 'verified':
        return 'verified';
      case 'pending':
        return 'pending DNS';
      case 'failed':
        return 'verification failed';
      default:
        return status;
    }
  }

  /** Verify is only meaningful when DNS control hasn't been proven yet. */
  function canVerify(d: CustomDomain): boolean {
    return d.status === 'pending' || d.status === 'failed';
  }

  /**
   * Copy-to-clipboard with a tiny inline indicator. Falls back to a manual
   * select+copy via a hidden textarea when the Clipboard API is unavailable
   * (older Safari, sandboxed iframes, non-secure contexts in some browsers).
   */
  let copied = $state<string | null>(null);
  async function copyToClipboard(value: string, key: string) {
    try {
      if (navigator?.clipboard?.writeText) {
        await navigator.clipboard.writeText(value);
      } else {
        const ta = document.createElement('textarea');
        ta.value = value;
        ta.setAttribute('readonly', '');
        ta.style.position = 'absolute';
        ta.style.left = '-9999px';
        document.body.appendChild(ta);
        ta.select();
        document.execCommand('copy');
        document.body.removeChild(ta);
      }
      copied = key;
      setTimeout(() => {
        if (copied === key) copied = null;
      }, 1500);
    } catch {
      // Best-effort: leave the value visible so the admin can manual-copy.
    }
  }

  function confirmRemove(host: string, e: MouseEvent) {
    const ok = confirm(
      `Remove custom domain "${host}"? Requests for this hostname will stop resolving immediately.`,
    );
    if (!ok) e.preventDefault();
  }
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Globe size="22" aria-hidden="true" /> Custom domain
  </h1>
  <p class="mt-1 max-w-2xl text-sm text-[var(--sp-muted-fg)]">
    Pin <code class="rounded bg-[var(--sp-muted)] px-1">skills.{data.tenant.slug}.com</code> (or any
    hostname you own) at this portal. After DNS verifies, the reverse proxy issues a Let's Encrypt
    cert automatically on the first request — see the
    <a
      class="underline"
      href="https://github.com/buildermethods/skill-pool/blob/main/docs/enterprise/custom-domains.md"
      rel="noopener">enterprise docs</a
    > for the end-to-end flow.
  </p>
</header>

<!-- ────────────────────────────────────────────────────────────────────
     Top-of-page toast / banner. Server actions set `form?.added`,
     `form?.removed`, `form?.verified`, or `form?.error`. Verify-failed
     gets a soft amber rather than red because the row's inline error is
     where the actual resolver message lives — duplicate red would be
     visually shouty.
     ──────────────────────────────────────────────────────────────────── -->
{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
    role="alert"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" aria-hidden="true" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.added}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
    role="status"
  >
    <CheckCircle2 size="16" class="mt-0.5 shrink-0" aria-hidden="true" />
    <span>
      Added <strong>{form.domain.hostname}</strong>. Next: add the verification TXT record below,
      then click <em>Verify now</em>.
    </span>
  </div>
{:else if form?.verified === true}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
    role="status"
  >
    <CheckCircle2 size="16" aria-hidden="true" />
    DNS verified — cert issuance happens on the next request to this hostname.
  </div>
{:else if form?.verified === false}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-900"
    role="status"
  >
    <AlertCircle size="16" class="mt-0.5 shrink-0" aria-hidden="true" />
    <span>
      Verification didn't pass — see the row below for the resolver's message. DNS records can take
      a few minutes to propagate after you add them.
    </span>
  </div>
{:else if form?.removed}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
    role="status"
  >
    <CheckCircle2 size="16" aria-hidden="true" /> Custom domain removed.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" aria-hidden="true" />
    <span>{data.error}</span>
  </div>
{/if}

<!-- ────────────────────────────────────────────────────────────────────
     CNAME instructions. Rendered above the table so admins see the
     target hostname before they even pick a name; the same target is
     what they paste into their DNS panel for every domain they add.
     ──────────────────────────────────────────────────────────────────── -->
<section
  class="mb-6 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
>
  <header class="mb-3 flex items-center gap-2">
    <ShieldCheck size="16" aria-hidden="true" />
    <h2 class="text-sm font-semibold">DNS setup</h2>
  </header>
  <p class="mb-3 max-w-2xl text-sm text-[var(--sp-muted-fg)]">
    Point a <code class="rounded bg-[var(--sp-bg)] px-1">CNAME</code> for the hostname you want to
    use (e.g.
    <code class="rounded bg-[var(--sp-bg)] px-1">skills.acme.com</code>) to:
  </p>
  <div
    class="flex flex-wrap items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] p-3"
  >
    <code class="grow font-mono text-sm break-all text-[var(--sp-fg)]">{data.cnameTarget}</code>
    <button
      type="button"
      onclick={() => copyToClipboard(data.cnameTarget, 'cname')}
      class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-2.5 py-1 text-xs hover:border-[var(--sp-primary)]"
      aria-label="Copy CNAME target to clipboard"
    >
      <Copy size="12" aria-hidden="true" />
      {copied === 'cname' ? 'Copied' : 'Copy'}
    </button>
  </div>
  {#if data.cnameTargetIsDefault}
    <p class="mt-2 text-xs text-[var(--sp-muted-fg)]">
      That's the docs placeholder. Ask your skill-pool admin to set
      <code>SKILL_POOL_PUBLIC_HOSTNAME</code> on the web server so this page shows your deployment's real
      front-door hostname.
    </p>
  {/if}
  <p class="mt-3 text-xs text-[var(--sp-muted-fg)]">
    You'll also be asked to add a one-shot
    <code class="rounded bg-[var(--sp-bg)] px-1">TXT</code> record so the server can prove you control
    the zone. The exact line is shown next to each pending domain below.
  </p>
</section>

<!-- ────────────────────────────────────────────────────────────────────
     Add-domain form. Hostname validation happens server-side; we just
     give the input a sensible placeholder + inline help.
     ──────────────────────────────────────────────────────────────────── -->
<section class="mb-6 rounded-[var(--sp-radius)] border border-[var(--sp-border)] p-4">
  <header class="mb-3 flex items-center gap-2">
    <Globe size="16" aria-hidden="true" />
    <h2 class="text-sm font-semibold">Add a domain</h2>
  </header>
  <form method="POST" action="?/add" class="flex flex-wrap items-end gap-3">
    <label class="grow">
      <span class="block text-xs font-medium text-[var(--sp-muted-fg)]">Hostname</span>
      <input
        type="text"
        name="hostname"
        required
        placeholder="skills.acme.com"
        autocomplete="off"
        spellcheck="false"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
        Fully-qualified ASCII hostname. Wildcards and IDNs aren't supported.
      </span>
    </label>
    <button
      type="submit"
      class="inline-flex items-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Add domain
    </button>
  </form>
</section>

<!-- ────────────────────────────────────────────────────────────────────
     Domains list. Pending rows surface their `verification_record` so
     the admin can copy it into their DNS panel inline.
     ──────────────────────────────────────────────────────────────────── -->
{#if data.domains.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    No custom domains configured. ACME issuance happens automatically once DNS verifies — add a
    hostname above to get started.
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
          <th class="px-4 py-3">Host</th>
          <th class="px-4 py-3">Status</th>
          <th class="px-4 py-3">Issued / activated</th>
          <th class="px-4 py-3">Last checked</th>
          <th class="px-4 py-3 text-right">Actions</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-[var(--sp-border)]">
        {#each data.domains as d (d.id)}
          <tr>
            <td class="px-4 py-3 align-top">
              <div class="font-mono text-xs break-all text-[var(--sp-fg)]">
                {d.hostname}
              </div>
              <div class="mt-1 text-xs text-[var(--sp-muted-fg)]">
                Added {fmtDate(d.created_at)}
              </div>
            </td>

            <td class="px-4 py-3 align-top">
              <span
                class="inline-flex items-center rounded-full px-2 py-0.5 text-xs font-medium {statusBadgeClass(
                  d.status,
                )}"
              >
                {statusLabel(d.status)}
              </span>
            </td>

            <td class="px-4 py-3 align-top text-xs text-[var(--sp-muted-fg)]">
              {#if d.activated_at}
                <span class="inline-flex items-center gap-1">
                  <ShieldCheck size="11" aria-hidden="true" />
                  {fmtDate(d.activated_at)}
                </span>
              {:else}
                —
              {/if}
            </td>

            <td class="px-4 py-3 align-top text-xs text-[var(--sp-muted-fg)]">
              {#if d.last_checked_at}
                <span class="inline-flex items-center gap-1">
                  <Clock size="11" aria-hidden="true" />
                  {fmtDate(d.last_checked_at)}
                </span>
              {:else}
                <span class="italic">never</span>
              {/if}
            </td>

            <td class="px-4 py-3 text-right align-top whitespace-nowrap">
              {#if canVerify(d)}
                <form method="POST" action="?/verify" class="inline-block">
                  <input type="hidden" name="id" value={d.id} />
                  <button
                    type="submit"
                    title="Run the DNS TXT lookup now"
                    class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-2.5 py-1 text-xs hover:border-[var(--sp-primary)]"
                  >
                    <RefreshCw size="12" aria-hidden="true" /> Verify now
                  </button>
                </form>
              {/if}
              <form method="POST" action="?/remove" class="ml-2 inline-block">
                <input type="hidden" name="id" value={d.id} />
                <button
                  type="submit"
                  onclick={(e) => confirmRemove(d.hostname, e)}
                  title="Withdraw this claim"
                  class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                >
                  <Trash2 size="12" aria-hidden="true" /> Remove
                </button>
              </form>
            </td>
          </tr>

          <!-- ────────────────────────────────────────────────────────────
               Pending / failed rows get an expanded detail row directly
               below so the TXT record (and any resolver error) is right
               where the admin is already looking. Active / verified
               rows keep the table compact.
               ──────────────────────────────────────────────────────── -->
          {#if d.status === 'pending' || d.status === 'failed'}
            <tr class="bg-[var(--sp-bg)]">
              <td colspan="5" class="px-4 py-3">
                <div class="space-y-3">
                  <div>
                    <div
                      class="mb-1 flex items-center gap-2 text-xs font-medium text-[var(--sp-fg)]"
                    >
                      <ShieldCheck size="12" aria-hidden="true" />
                      Verification TXT record
                    </div>
                    <div
                      class="flex flex-wrap items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-2"
                    >
                      <code class="grow font-mono text-xs break-all text-[var(--sp-fg)]"
                        >{d.verification_record}</code
                      >
                      <button
                        type="button"
                        onclick={() => copyToClipboard(d.verification_record, `txt-${d.id}`)}
                        class="inline-flex items-center gap-1.5 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-2 py-0.5 text-xs hover:border-[var(--sp-primary)]"
                        aria-label="Copy verification TXT record"
                      >
                        <Copy size="11" aria-hidden="true" />
                        {copied === `txt-${d.id}` ? 'Copied' : 'Copy'}
                      </button>
                    </div>
                  </div>

                  {#if d.last_error}
                    <div
                      class="flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-200 bg-red-50 p-2 text-xs text-red-800"
                      role="alert"
                    >
                      <AlertCircle size="12" class="mt-0.5 shrink-0" aria-hidden="true" />
                      <span class="font-mono break-words whitespace-pre-wrap">{d.last_error}</span>
                    </div>
                  {/if}
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>

  <!-- Cert-status footnote: the server-side wire shape doesn't carry an
       explicit `cert_status` field today, so we surface what we have:
       `activated_at` tells the admin when DNS proof landed; the proxy
       then issues the cert on the next request to that host. We
       deliberately don't pretend to know about cert issuance — the
       reverse proxy is authoritative there. -->
  <p class="mt-3 text-xs text-[var(--sp-muted-fg)]">
    Cert issuance happens automatically the first time a user requests an
    <strong>active</strong> or <strong>verified</strong> hostname. Until then the row sits in
    <strong>pending DNS</strong>. The server doesn't re-poll DNS after verification — to re-prove
    control, remove and re-add the domain.
  </p>
{/if}
