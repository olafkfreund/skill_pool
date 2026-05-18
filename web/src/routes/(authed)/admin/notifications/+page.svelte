<script lang="ts">
  import { AlertTriangle, Bell, CheckCircle2, Lock } from '@lucide/svelte';

  let { data, form } = $props();

  const current = $derived(form?.saved ? form.config : data.config);
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Bell size="22" /> Curator notifications
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Wire a webhook so the team learns when a new draft lands in the inbox. Works with Slack and
    Discord incoming-webhook URLs out of the box — they accept the top-level
    <code class="rounded bg-[var(--sp-muted)] px-1">text</code> field. Custom endpoints get the same
    payload plus structured <code class="rounded bg-[var(--sp-muted)] px-1">event</code> /
    <code class="rounded bg-[var(--sp-muted)] px-1">tenant</code> /
    <code class="rounded bg-[var(--sp-muted)] px-1">draft</code> fields.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.saved}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Saved.
  </div>
{/if}

<form method="POST" action="?/save" class="max-w-2xl space-y-4">
  <label class="block">
    <span class="text-sm font-medium text-[var(--sp-fg)]">Webhook URL</span>
    <input
      type="url"
      name="webhook_url"
      value={current?.webhook_url ?? ''}
      placeholder="https://hooks.slack.com/services/…"
      class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
    <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
      Leave empty to disable notifications.
    </span>
  </label>

  <fieldset class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] p-3">
    <legend class="px-1 text-xs font-medium text-[var(--sp-muted-fg)]">
      <Lock size="11" class="inline" /> Signing secret (optional)
    </legend>
    <p class="mb-2 text-xs text-[var(--sp-muted-fg)]">
      When set, every delivery carries an <code>X-Skill-Pool-Signature: sha256=&lt;hex&gt;</code> header
      so the receiver can verify the body wasn't tampered with. Same convention as GitHub and Stripe.
    </p>
    <input
      type="password"
      name="webhook_secret"
      placeholder={current?.signing_enabled ? '••••••••  (leave empty to keep current)' : 'set a secret…'}
      autocomplete="new-password"
      class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
    />
    {#if current?.signing_enabled}
      <label class="mt-2 inline-flex cursor-pointer items-center gap-2 text-xs text-[var(--sp-muted-fg)]">
        <input
          type="checkbox"
          name="clear_secret"
          value="1"
          class="h-3.5 w-3.5 rounded border-[var(--sp-border)]"
        />
        Remove the existing secret (deliveries become unsigned).
      </label>
    {/if}
  </fieldset>

  <button
    type="submit"
    class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
  >
    Save
  </button>
</form>
