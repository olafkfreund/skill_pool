<script lang="ts">
  import { AlertTriangle, CheckCircle2, Copy, KeyRound, Trash2, X } from '@lucide/svelte';
  import { enhance } from '$app/forms';
  import type { ApiToken } from '$lib/server/api';

  let { data, form } = $props();

  const SCOPE_OPTIONS = [
    {
      value: 'skills:read',
      label: 'skills:read',
      help: 'Read the catalog, fetch bundles.',
      restricted: false,
    },
    {
      value: 'skills:publish',
      label: 'skills:publish',
      help: 'Publish + retire skills you own.',
      restricted: false,
    },
    {
      value: 'tenant:admin',
      label: 'tenant:admin',
      help: 'Full admin capabilities. Only admins may mint.',
      restricted: true,
    },
  ] as const;

  // Default selection mirrors the most common CLI use case.
  let selectedScopes = $state(new Set<string>(['skills:read', 'skills:publish']));
  let label = $state('');
  let copied = $state(false);
  let modalDismissed = $state(false);

  const isAdmin = $derived(data.identity?.role === 'admin');
  const created = $derived(form && 'created' in form ? form.created : null);
  const showModal = $derived(!!created && !modalDismissed);

  function toggleScope(value: string) {
    if (selectedScopes.has(value)) {
      selectedScopes.delete(value);
    } else {
      selectedScopes.add(value);
    }
    // Force reactivity — Set mutation alone doesn't trigger Svelte 5's
    // proxy.
    selectedScopes = new Set(selectedScopes);
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

  function fmtRelative(iso: string | null): string {
    if (!iso) return '—';
    try {
      const ts = new Date(iso).getTime();
      const diffSecs = Math.round((Date.now() - ts) / 1000);
      if (diffSecs < 60) return 'just now';
      if (diffSecs < 3600) return `${Math.floor(diffSecs / 60)}m ago`;
      if (diffSecs < 86400) return `${Math.floor(diffSecs / 3600)}h ago`;
      const days = Math.floor(diffSecs / 86400);
      return days < 30 ? `${days}d ago` : fmtDate(iso);
    } catch {
      return iso;
    }
  }

  function isRevoked(t: ApiToken): boolean {
    return t.revoked_at !== null;
  }

  async function copyToken(value: string) {
    try {
      await navigator.clipboard.writeText(value);
      copied = true;
      setTimeout(() => (copied = false), 2000);
    } catch {
      copied = false;
    }
  }

  function closeModal() {
    modalDismissed = true;
    // Reset the form so a refresh/back doesn't re-show the token.
    label = '';
    selectedScopes = new Set(['skills:read', 'skills:publish']);
  }
</script>

<header class="mb-6">
  <h1 class="text-2xl font-semibold">Profile</h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Your identity and personal API tokens for <code class="rounded bg-[var(--sp-muted)] px-1"
      >{data.tenant.slug}</code
    >.
  </p>
</header>

{#if data.identity}
  <section class="mb-8">
    <div
      class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
    >
      <dl class="grid grid-cols-1 gap-3 text-sm sm:grid-cols-3">
        <div>
          <dt class="text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">
            Email
          </dt>
          <dd class="mt-1 font-mono text-[var(--sp-fg)]">{data.identity.email}</dd>
        </div>
        <div>
          <dt class="text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">
            Role
          </dt>
          <dd class="mt-1 text-[var(--sp-fg)]">
            <span class="rounded-full bg-slate-100 px-2 py-0.5 text-xs font-medium text-slate-700">
              {data.identity.role}
            </span>
          </dd>
        </div>
        <div>
          <dt class="text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">
            Tenant
          </dt>
          <dd class="mt-1 font-mono text-[var(--sp-fg)]">{data.identity.tenant}</dd>
        </div>
      </dl>
    </div>
  </section>
{/if}

<section>
  <div class="mb-3 flex items-center justify-between">
    <h2 class="flex items-center gap-2 text-lg font-semibold">
      <KeyRound size="18" /> API tokens
    </h2>
  </div>
  <p class="mb-4 text-sm text-[var(--sp-muted-fg)]">
    Personal credentials for the CLI and machine-to-machine integrations. The raw token is shown
    once at creation — store it somewhere safe (a secret manager, not source control).
  </p>

  {#if form?.error}
    <div
      class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
    >
      <AlertTriangle size="16" class="mt-0.5 shrink-0" />
      <span class="break-words whitespace-pre-wrap">{form.error}</span>
    </div>
  {:else if form && 'revoked' in form && form.revoked}
    <div
      class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
    >
      <CheckCircle2 size="16" /> Token revoked.
    </div>
  {:else if data.loadError}
    <div
      class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
    >
      <AlertTriangle size="16" class="mt-0.5 shrink-0" />
      <span>{data.loadError}</span>
    </div>
  {/if}

  <!-- Create form -->
  {#if !data.loadError}
    <form
      method="POST"
      action="?/create"
      use:enhance={() => {
        modalDismissed = false;
        return async ({ update }) => update();
      }}
      class="mb-6 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-4"
    >
      <h3 class="mb-3 text-sm font-semibold">Create a new token</h3>
      <div class="grid gap-3 md:grid-cols-2">
        <label class="flex flex-col gap-1">
          <span class="text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">
            Label
          </span>
          <input
            type="text"
            name="label"
            bind:value={label}
            required
            maxlength="80"
            placeholder="e.g. laptop CLI, CI runner"
            class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm text-[var(--sp-fg)] focus:border-[var(--sp-primary)] focus:outline-none"
          />
        </label>
        <fieldset class="flex flex-col gap-1">
          <legend class="text-xs font-medium tracking-wide text-[var(--sp-muted-fg)] uppercase">
            Scopes
          </legend>
          <div class="flex flex-wrap gap-3 text-sm">
            {#each SCOPE_OPTIONS as opt (opt.value)}
              {@const disabled = opt.restricted && !isAdmin}
              <label
                class="flex items-start gap-2 {disabled
                  ? 'cursor-not-allowed opacity-50'
                  : 'cursor-pointer'}"
                title={disabled
                  ? 'Only tenant admins may mint this scope.'
                  : opt.help}
              >
                <input
                  type="checkbox"
                  name="scopes"
                  value={opt.value}
                  checked={selectedScopes.has(opt.value)}
                  onchange={() => toggleScope(opt.value)}
                  {disabled}
                  class="mt-0.5"
                />
                <span>
                  <code class="font-mono text-xs">{opt.label}</code>
                  <span class="block text-xs text-[var(--sp-muted-fg)]">{opt.help}</span>
                </span>
              </label>
            {/each}
          </div>
        </fieldset>
      </div>
      <div class="mt-4 flex justify-end">
        <button
          type="submit"
          class="rounded-[var(--sp-radius)] border border-transparent px-4 py-2 text-sm font-medium text-[var(--sp-primary-fg)]"
          style="background: var(--sp-primary);"
          disabled={label.trim().length === 0 || selectedScopes.size === 0}
        >
          Create token
        </button>
      </div>
    </form>
  {/if}

  <!-- Token list -->
  {#if data.tokens.length === 0 && !data.loadError}
    <div
      class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
    >
      No tokens yet. Create one above to use with <code class="font-mono">skill-pool</code> from the
      CLI or CI.
    </div>
  {:else if data.tokens.length > 0}
    <div
      class="overflow-hidden rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)]"
    >
      <table class="w-full text-sm">
        <thead
          class="bg-[var(--sp-bg)] text-left text-xs tracking-wide text-[var(--sp-muted-fg)] uppercase"
        >
          <tr>
            <th class="px-4 py-3">Label</th>
            <th class="px-4 py-3">Prefix</th>
            <th class="px-4 py-3">Scopes</th>
            <th class="px-4 py-3">Created</th>
            <th class="px-4 py-3">Last used</th>
            <th class="px-4 py-3">Status</th>
            <th class="px-4 py-3 text-right">Actions</th>
          </tr>
        </thead>
        <tbody class="divide-y divide-[var(--sp-border)]">
          {#each data.tokens as t (t.id)}
            {@const revoked = isRevoked(t)}
            <tr class={revoked ? 'opacity-50' : ''}>
              <td class="px-4 py-3 text-[var(--sp-fg)]">{t.label}</td>
              <td class="px-4 py-3 font-mono text-xs text-[var(--sp-muted-fg)]">
                {t.prefix ?? '—'}
              </td>
              <td class="px-4 py-3 font-mono text-xs text-[var(--sp-muted-fg)]">{t.scopes}</td>
              <td class="px-4 py-3 text-[var(--sp-muted-fg)]">{fmtDate(t.created_at)}</td>
              <td class="px-4 py-3 text-[var(--sp-muted-fg)]">{fmtRelative(t.last_used_at)}</td>
              <td class="px-4 py-3">
                {#if revoked}
                  <span
                    class="rounded-full bg-red-100 px-2 py-0.5 text-xs font-medium text-red-700"
                    title={t.revoked_at ?? ''}
                  >
                    revoked
                  </span>
                {:else}
                  <span
                    class="rounded-full bg-emerald-100 px-2 py-0.5 text-xs font-medium text-emerald-700"
                  >
                    active
                  </span>
                {/if}
              </td>
              <td class="px-4 py-3 text-right">
                {#if !revoked}
                  <form
                    method="POST"
                    action="?/revoke"
                    use:enhance
                    class="inline-block"
                  >
                    <input type="hidden" name="id" value={t.id} />
                    <button
                      type="submit"
                      title="Revoke this token"
                      class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                    >
                      <Trash2 size="12" /> Revoke
                    </button>
                  </form>
                {:else}
                  <span class="text-xs text-[var(--sp-muted-fg)]">—</span>
                {/if}
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  {/if}
</section>

<!-- One-time raw-token reveal modal -->
{#if showModal && created}
  <div
    class="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4"
    role="dialog"
    aria-modal="true"
    aria-labelledby="token-modal-title"
  >
    <div
      class="w-full max-w-lg rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] p-6 shadow-2xl"
    >
      <div class="mb-3 flex items-start justify-between">
        <h2 id="token-modal-title" class="text-lg font-semibold">Token created</h2>
        <button
          type="button"
          onclick={closeModal}
          class="rounded-[var(--sp-radius)] p-1 text-[var(--sp-muted-fg)] hover:bg-[var(--sp-muted)] hover:text-[var(--sp-fg)]"
          aria-label="Close"
        >
          <X size="18" />
        </button>
      </div>
      <p class="mb-4 text-sm text-[var(--sp-muted-fg)]">
        Copy this token now — it will not be shown again. Only its hash is stored.
      </p>
      <div
        class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-3"
      >
        <code class="flex-1 overflow-x-auto font-mono text-xs break-all text-[var(--sp-fg)]"
          >{created.raw_token}</code
        >
        <button
          type="button"
          onclick={() => copyToken(created.raw_token)}
          class="inline-flex shrink-0 items-center gap-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-2 py-1 text-xs hover:border-[var(--sp-primary)]"
        >
          <Copy size="12" />
          {copied ? 'Copied' : 'Copy'}
        </button>
      </div>
      <dl class="mb-4 grid grid-cols-2 gap-3 text-xs">
        <div>
          <dt class="text-[var(--sp-muted-fg)]">Label</dt>
          <dd class="mt-0.5 font-mono">{created.label}</dd>
        </div>
        <div>
          <dt class="text-[var(--sp-muted-fg)]">Scopes</dt>
          <dd class="mt-0.5 font-mono">{created.scopes}</dd>
        </div>
      </dl>
      <div class="flex justify-end">
        <button
          type="button"
          onclick={closeModal}
          class="rounded-[var(--sp-radius)] border border-transparent px-4 py-2 text-sm font-medium text-[var(--sp-primary-fg)]"
          style="background: var(--sp-primary);"
        >
          I've saved it
        </button>
      </div>
    </div>
  </div>
{/if}
