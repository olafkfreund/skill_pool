<script lang="ts">
  import { LogIn, ShieldCheck } from '@lucide/svelte';

  let { data, form } = $props();
</script>

<section
  class="w-full max-w-md rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-8 shadow-sm"
>
  <header class="mb-6 text-center">
    <h1 class="text-2xl font-semibold text-[var(--sp-fg)]">
      Sign in to <span style="color: var(--sp-primary)">{data.tenant.slug}</span>
    </h1>
    <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
      {data.sso?.enabled ? 'SSO is enabled for this tenant.' : 'API token sign-in.'}
    </p>
  </header>

  {#if data.oidcStart}
    <a
      href={data.oidcStart}
      class="mb-4 inline-flex w-full items-center justify-center gap-2 rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm font-medium hover:border-[var(--sp-primary)]"
    >
      <ShieldCheck size="16" /> Sign in with SSO
    </a>
    <div class="my-4 flex items-center gap-2 text-xs text-[var(--sp-muted-fg)]">
      <span class="h-px flex-1 bg-[var(--sp-border)]"></span>
      or paste an API token
      <span class="h-px flex-1 bg-[var(--sp-border)]"></span>
    </div>
  {/if}

  <form method="POST" class="space-y-4">
    <label class="block text-sm">
      <span class="text-[var(--sp-fg)]">API token</span>
      <input
        type="password"
        name="token"
        autocomplete="current-password"
        required
        placeholder="spk_…"
        class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-[var(--sp-fg)] focus:border-[var(--sp-primary)] focus:outline-none"
      />
    </label>

    {#if form?.error}
      <p class="text-sm text-red-500">{form.error}</p>
    {/if}

    <button
      type="submit"
      class="inline-flex w-full items-center justify-center gap-2 rounded-[var(--sp-radius)] px-4 py-2 font-medium"
      style="background: var(--sp-primary); color: var(--sp-primary-fg);"
    >
      <LogIn size="16" /> Sign in
    </button>
  </form>

  <p class="mt-6 text-center text-xs text-[var(--sp-muted-fg)]">
    Get a token: <code class="rounded bg-[var(--sp-bg)] px-1 py-0.5"
      >skill-pool-server admin token-create</code
    >
  </p>
</section>
