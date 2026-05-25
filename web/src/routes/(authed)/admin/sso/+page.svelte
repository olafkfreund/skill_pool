<script lang="ts">
  import { untrack } from 'svelte';
  import {
    AlertTriangle,
    CheckCircle2,
    Eye,
    EyeOff,
    KeyRound,
    Network,
    ShieldCheck,
    Trash2,
  } from '@lucide/svelte';
  import { SSO_ROLES } from '$lib/sso-roles';

  let { data, form } = $props();

  // The action returns a fresh config on save / clear; fall back to the
  // loader value otherwise. This keeps the form in sync with what the
  // server actually persisted, including the masked secret hint.
  const config = $derived(form?.config ?? data.config);

  // The OIDC and SAML rows live in separate DB tables, so a tenant can
  // technically configure both. The UI funnels callers into a single
  // active provider by hiding the non-selected tab, but we don't *force*
  // exclusivity at the API layer — operators can pre-populate via CLI.
  // Seed the initial tab from the loader's config (the form action's
  // returned config can't change the kind retroactively — a SAML save
  // lands you on the SAML tab because you were already there).
  let activeTab = $state<'oidc' | 'saml'>(
    untrack(() => (data.config?.kind === 'saml' ? 'saml' : 'oidc')),
  );
  let showSecret = $state(false);

  function fmtTested(t: typeof form): string {
    if (!t?.tested) return '';
    return t.tested === 'saml' ? 'SAML' : 'OIDC';
  }
</script>

<header class="mb-6">
  <h1 class="flex items-center gap-2 text-2xl font-semibold">
    <ShieldCheck size="22" /> Single sign-on
  </h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    Configure how members of <code class="rounded bg-[var(--sp-muted)] px-1"
      >{data.tenant.slug}</code
    > sign in. Pick OIDC for OpenID Connect IdPs (Okta, Auth0, Keycloak, Authentik, Azure AD) or SAML
    2.0 for IdPs that don't speak OIDC. SCIM provisioning runs independently of the sign-in protocol.
  </p>
</header>

{#if form?.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-red-300 bg-red-50 p-3 text-sm text-red-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span class="break-words whitespace-pre-wrap">{form.error}</span>
  </div>
{:else if form?.saved === 'oidc'}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> OIDC configuration saved.
  </div>
{:else if form?.saved === 'saml'}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> SAML configuration saved.
  </div>
{:else if form?.cleared}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> SSO configuration cleared.
  </div>
{:else if form?.tested}
  {#if form.ok}
    <div
      class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
    >
      <CheckCircle2 size="16" />
      {fmtTested(form)} IdP discovery succeeded.
    </div>
  {:else}
    <div
      class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
    >
      <AlertTriangle size="16" class="mt-0.5 shrink-0" />
      <span class="break-words whitespace-pre-wrap">
        {fmtTested(form)} discovery failed: {form.error ?? 'unknown error'}
      </span>
    </div>
  {/if}
{/if}

<!-- Tab strip -->
<div class="mb-4 flex gap-2 border-b border-[var(--sp-border)]">
  <button
    type="button"
    onclick={() => (activeTab = 'oidc')}
    class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {activeTab === 'oidc'
      ? 'border-[var(--sp-primary)] text-[var(--sp-fg)]'
      : 'border-transparent text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]'}"
  >
    <KeyRound size="14" class="mr-1 inline" /> OIDC
    {#if config?.kind === 'oidc'}
      <span class="ml-1 rounded-full bg-emerald-100 px-1.5 text-xs text-emerald-800">active</span>
    {/if}
  </button>
  <button
    type="button"
    onclick={() => (activeTab = 'saml')}
    class="-mb-px border-b-2 px-3 py-2 text-sm font-medium {activeTab === 'saml'
      ? 'border-[var(--sp-primary)] text-[var(--sp-fg)]'
      : 'border-transparent text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]'}"
  >
    <Network size="14" class="mr-1 inline" /> SAML 2.0
    {#if config?.kind === 'saml'}
      <span class="ml-1 rounded-full bg-emerald-100 px-1.5 text-xs text-emerald-800">active</span>
    {/if}
  </button>
</div>

{#if activeTab === 'oidc'}
  <section class="mb-10">
    <p class="mb-4 max-w-2xl text-sm text-[var(--sp-muted-fg)]">
      Paste your IdP's issuer URL and the client credentials it issued for skill-pool. The server
      discovers <code class="rounded bg-[var(--sp-muted)] px-1"
        >.well-known/openid-configuration</code
      >
      at the issuer URL, so any spec-compliant IdP works without per-vendor wiring. Members sign in at
      <code class="rounded bg-[var(--sp-muted)] px-1">/v1/auth/oidc/&lt;tenant&gt;/start</code>.
    </p>

    <form method="POST" action="?/saveOidc" class="max-w-2xl space-y-4">
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">Issuer URL</span>
        <input
          type="url"
          name="issuer_url"
          value={config?.oidc?.issuer_url ?? ''}
          placeholder="https://login.example.com/realms/acme"
          required
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          The URL that hosts <code>.well-known/openid-configuration</code>. No trailing slash.
        </span>
      </label>

      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">Client ID</span>
        <input
          type="text"
          name="client_id"
          value={config?.oidc?.client_id ?? ''}
          placeholder="skill-pool-spk"
          required
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
      </label>

      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">Client secret</span>
        <div class="mt-1 flex gap-2">
          <input
            type={showSecret ? 'text' : 'password'}
            name="client_secret"
            placeholder={config?.oidc?.client_secret_hint
              ? `current: ${config.oidc.client_secret_hint}  (paste to replace)`
              : 'paste from IdP…'}
            autocomplete="new-password"
            required
            class="flex-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
          />
          <button
            type="button"
            onclick={() => (showSecret = !showSecret)}
            title={showSecret ? 'Hide' : 'Show'}
            class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-2 text-sm hover:border-[var(--sp-primary)]"
          >
            {#if showSecret}<EyeOff size="14" />{:else}<Eye size="14" />{/if}
          </button>
        </div>
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          Sent verbatim to the IdP's token endpoint during code exchange. Never shown after save —
          you'll only see the last 4 chars.
        </span>
      </label>

      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">Default role for new members</span>
        <select
          name="default_role"
          value={config?.oidc?.default_role ?? 'viewer'}
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        >
          {#each SSO_ROLES as r (r)}
            <option value={r}>{r}</option>
          {/each}
        </select>
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          IdP groups can override this per-user via the role-mapping table.
        </span>
      </label>

      <div class="flex flex-wrap gap-2">
        <button
          type="submit"
          class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
          style="background: var(--sp-primary); color: var(--sp-primary-fg);"
        >
          Save OIDC
        </button>
        <button
          type="submit"
          formaction="?/test"
          name="which"
          value="oidc"
          class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm font-medium hover:border-[var(--sp-primary)]"
        >
          Test discovery
        </button>
      </div>
    </form>
  </section>
{:else}
  <section class="mb-10">
    <p class="mb-4 max-w-2xl text-sm text-[var(--sp-muted-fg)]">
      Paste the full <code class="rounded bg-[var(--sp-muted)] px-1">EntityDescriptor</code> XML
      published by your IdP (e.g.
      <code class="rounded bg-[var(--sp-muted)] px-1">https://idp/saml2/idp/metadata.php</code>). We
      extract the entity ID, SSO URL, and signing certificate. Hand your IdP admin our metadata at
      <code class="rounded bg-[var(--sp-muted)] px-1 break-all">{data.samlMetadataUrl}</code>.
    </p>

    {#if config?.saml}
      <dl
        class="mb-4 grid max-w-2xl grid-cols-[max-content_1fr] gap-x-4 gap-y-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-3 text-xs"
      >
        <dt class="text-[var(--sp-muted-fg)]">IdP entity</dt>
        <dd class="font-mono break-all">{config.saml.idp_entity_id}</dd>
        <dt class="text-[var(--sp-muted-fg)]">IdP SSO URL</dt>
        <dd class="font-mono break-all">{config.saml.idp_sso_url}</dd>
        <dt class="text-[var(--sp-muted-fg)]">Signing cert</dt>
        <dd>{config.saml.idp_x509_cert_bytes} bytes (PEM)</dd>
        <dt class="text-[var(--sp-muted-fg)]">Default role</dt>
        <dd>{config.saml.default_role}</dd>
      </dl>
    {/if}

    <form method="POST" action="?/saveSaml" class="max-w-2xl space-y-4">
      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">IdP metadata XML</span>
        <textarea
          name="metadata_xml"
          required
          rows="10"
          placeholder={'<?xml version="1.0"?>\n<EntityDescriptor entityID="https://idp.example.com/..." ...>\n  <IDPSSODescriptor ...>\n    <KeyDescriptor use="signing">...</KeyDescriptor>\n    <SingleSignOnService Location="..." Binding="..."/>\n  </IDPSSODescriptor>\n</EntityDescriptor>'}
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-xs focus:border-[var(--sp-primary)] focus:outline-none"
        ></textarea>
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          We validate that the document parses and that it contains an IDPSSODescriptor with at
          least one SingleSignOnService and one signing certificate.
        </span>
      </label>

      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">SP entity ID (optional)</span>
        <input
          type="text"
          name="sp_entity_id"
          value={config?.saml?.sp_entity_id ?? ''}
          placeholder="urn:skill-pool:tenant:acme  (default)"
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 font-mono text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        />
        <span class="mt-1 block text-xs text-[var(--sp-muted-fg)]">
          Override only if your IdP demands a specific SP entity ID different from our default.
        </span>
      </label>

      <label class="block">
        <span class="text-sm font-medium text-[var(--sp-fg)]">Default role for new members</span>
        <select
          name="default_role"
          value={config?.saml?.default_role ?? 'viewer'}
          class="mt-1 w-full rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-3 py-2 text-sm focus:border-[var(--sp-primary)] focus:outline-none"
        >
          {#each SSO_ROLES as r (r)}
            <option value={r}>{r}</option>
          {/each}
        </select>
      </label>

      <div class="flex flex-wrap gap-2">
        <button
          type="submit"
          class="rounded-[var(--sp-radius)] px-4 py-2 text-sm font-medium"
          style="background: var(--sp-primary); color: var(--sp-primary-fg);"
        >
          Save SAML
        </button>
        <button
          type="submit"
          formaction="?/test"
          name="which"
          value="saml"
          class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-4 py-2 text-sm font-medium hover:border-[var(--sp-primary)]"
        >
          Test discovery
        </button>
      </div>
    </form>
  </section>
{/if}

<!-- Danger zone — applies regardless of active tab -->
{#if config?.kind}
  <section class="mb-10 max-w-2xl">
    <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
      Danger zone
    </h2>
    <form
      method="POST"
      action="?/clear"
      onsubmit={(e) => {
        if (
          !confirm(
            'Clear all SSO configuration for this tenant? Existing user_sessions stay valid until they expire.',
          )
        ) {
          e.preventDefault();
        }
      }}
      class="flex items-center justify-between rounded-[var(--sp-radius)] border border-red-200 bg-red-50 p-3 text-sm"
    >
      <span class="text-red-800">
        Remove the current {config.kind === 'oidc' ? 'OIDC' : 'SAML'} configuration. Members will no longer
        be able to start an SSO sign-in.
      </span>
      <button
        type="submit"
        class="ml-3 inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-300 bg-white px-3 py-1.5 text-xs font-medium text-red-700 hover:bg-red-100"
      >
        <Trash2 size="12" /> Clear SSO
      </button>
    </form>
  </section>
{/if}

<!-- SCIM details -->
<section class="max-w-2xl">
  <h2 class="mb-2 text-sm font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase">
    SCIM 2.0 provisioning
  </h2>
  <p class="mb-3 text-sm text-[var(--sp-muted-fg)]">
    Optional. Point your IdP's user-provisioning at the endpoint below using a bearer token with the <code
      class="rounded bg-[var(--sp-muted)] px-1">scim:provision</code
    > scope. SCIM works whether OIDC or SAML is configured — or even when neither is.
  </p>
  <dl
    class="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-muted)] p-3 text-xs"
  >
    <dt class="text-[var(--sp-muted-fg)]">Base URL</dt>
    <dd class="font-mono break-all">{config?.scim_endpoint ?? '/scim/v2/Users'}</dd>
    <dt class="text-[var(--sp-muted-fg)]">Bearer token</dt>
    <dd>
      <!-- Token CRUD is a separate slice (δ); show a placeholder rather than half-implement. -->
      <span class="text-[var(--sp-muted-fg)]">
        Mint via <code class="rounded bg-[var(--sp-bg)] px-1"
          >skill-pool-server admin token-create --scope 'scim:provision'</code
        > (UI token management ships in a follow-up slice).
      </span>
    </dd>
  </dl>
</section>
