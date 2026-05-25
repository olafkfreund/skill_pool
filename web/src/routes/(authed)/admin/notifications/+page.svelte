<script lang="ts">
  import { AlertTriangle, Bell, CheckCircle2, Lock, Mail, Webhook } from '@lucide/svelte';

  let { data, form } = $props();

  // After a save, the action returns the fresh config so the form
  // reflects what just landed. Otherwise use the loader's value.
  const current = $derived(form?.config ?? data.config);
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <Bell size="22" /> Curator notifications
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Two delivery channels — wire either or both. Both fire on
    <code class="rounded bg-[var(--sp-muted)] px-1">draft.created</code> in parallel; either can fail
    without affecting the other (every attempt lands in the audit log).
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.saved === 'webhook'}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Webhook settings saved.
  </div>
{:else if form?.saved === 'email'}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Email settings saved.
  </div>
{/if}

<!-- Webhook section -->
<section class="mb-10">
  <h2
    class="mb-3 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
  >
    <Webhook size="13" /> Webhook
  </h2>
  <p class="mb-3 max-w-2xl text-sm text-[var(--sp-muted-fg)]">
    Slack / Discord incoming-webhook URLs work out of the box — they accept the top-level <code
      class="rounded bg-[var(--sp-muted)] px-1">text</code
    > field. Custom endpoints get the same payload plus structured fields.
  </p>

  <form method="POST" action="?/saveWebhook" class="max-w-2xl space-y-4">
    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">Webhook URL</span>
      <input
        type="url"
        name="webhook_url"
        value={current?.webhook_url ?? ''}
        placeholder="https://hooks.slack.com/services/…"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]"> Leave empty to disable. </span>
    </label>

    <fieldset class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] p-3">
      <legend class="px-1 text-xs font-medium text-[var(--sp-muted-fg)]">
        <Lock size="11" class="inline" /> Signing secret (optional)
      </legend>
      <p class="mb-2 text-xs text-[var(--sp-muted-fg)]">
        When set, every delivery carries an
        <code>X-Skill-Pool-Signature: sha256=&lt;hex&gt;</code> header so the receiver can verify the
        body wasn't tampered with. Same convention as GitHub and Stripe.
      </p>
      <input
        type="password"
        name="webhook_secret"
        placeholder={current?.signing_enabled
          ? '••••••••  (leave empty to keep current)'
          : 'set a secret…'}
        autocomplete="new-password"
        class="w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      {#if current?.signing_enabled}
        <label
          class="mt-2 inline-flex cursor-pointer items-center gap-2 text-xs text-[var(--sp-muted-fg)]"
        >
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
      Save webhook
    </button>
  </form>
</section>

<!-- Email section -->
<section>
  <h2
    class="mb-3 flex items-center gap-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
  >
    <Mail size="13" /> Email (SMTP)
  </h2>
  <p class="mb-3 max-w-2xl text-sm text-[var(--sp-muted-fg)]">
    Configure an SMTP relay you operate (Postfix, SES, Mailgun, SendGrid, …). All three fields must
    be set for email delivery to fire. The To address is typically a distribution list at your
    domain.
  </p>

  <form method="POST" action="?/saveEmail" class="max-w-2xl space-y-4">
    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">SMTP URL</span>
      <input
        type="text"
        name="smtp_url"
        value={current?.smtp_url ?? ''}
        placeholder="smtps://user:pass@smtp.example.com:465"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
        <code>smtp://</code> = plain SUBMIT (often STARTTLS to upgrade);
        <code>smtps://</code> = implicit TLS on port 465. Leave empty to disable email delivery.
      </span>
    </label>

    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">From</span>
      <input
        type="text"
        name="smtp_from"
        value={current?.smtp_from ?? ''}
        placeholder="skill-pool <noreply@example.com>"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
        Standard RFC 5322 mailbox: <code>Name &lt;addr@host&gt;</code> or bare
        <code>addr@host</code>.
      </span>
    </label>

    <label class="block">
      <span class="text-sm font-medium text-[var(--sp-fg)]">To</span>
      <input
        type="text"
        name="smtp_to"
        value={current?.smtp_to ?? ''}
        placeholder="curators@example.com"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
      />
      <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
        One address. Use a distribution list at your domain to fan out to multiple curators.
      </span>
    </label>

    <button
      type="submit"
      class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      Save email
    </button>
  </form>

  <p class="mt-4 max-w-2xl text-xs text-[var(--sp-muted-fg)]">
    Deliverability (SPF / DKIM / list-unsubscribe headers) is your relay's responsibility.
    skill-pool ships well-formed RFC 5322 messages with proper From / To / Subject / Date /
    Message-ID headers and lets the relay add the rest.
  </p>
</section>
