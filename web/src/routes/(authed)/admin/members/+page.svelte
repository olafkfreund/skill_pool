<script lang="ts">
  import { AlertTriangle, CheckCircle2, Trash2 } from '@lucide/svelte';
  import type { Member } from '$lib/server/api';

  let { data, form } = $props();

  const ROLES = ['viewer', 'publisher', 'curator', 'admin'] as const;

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

  function badgeClass(role: Member['role']): string {
    return role === 'admin'
      ? 'bg-purple-100 text-purple-800'
      : role === 'curator'
        ? 'bg-amber-100 text-amber-800'
        : role === 'publisher'
          ? 'bg-sky-100 text-sky-800'
          : 'bg-slate-100 text-slate-700';
  }
</script>

<header class="mb-6">
  <h1 class="text-2xl font-semibold">Members</h1>
  <p class="mt-1 text-sm text-[var(--sp-muted-fg)]">
    People with access to <code class="rounded bg-[var(--sp-muted)] px-1">{data.tenant.slug}</code>.
    Provision via SCIM (Okta / Azure AD) or via the admin CLI; this page is the canonical place to
    change someone's role or revoke access.
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
    <CheckCircle2 size="16" /> Role updated.
  </div>
{:else if form?.removed}
  <div
    class="mb-4 flex items-center gap-2 rounded-[var(--sp-radius)] border border-emerald-300 bg-emerald-50 p-3 text-sm text-emerald-800"
  >
    <CheckCircle2 size="16" /> Member removed.
  </div>
{:else if 'error' in data && data.error}
  <div
    class="mb-4 flex items-start gap-2 rounded-[var(--sp-radius)] border border-amber-300 bg-amber-50 p-3 text-sm text-amber-800"
  >
    <AlertTriangle size="16" class="mt-0.5 shrink-0" />
    <span>{data.error}</span>
  </div>
{/if}

{#if data.members.length === 0}
  <div
    class="rounded-[var(--sp-radius)] border border-dashed border-[var(--sp-border)] p-12 text-center text-sm text-[var(--sp-muted-fg)]"
  >
    No members yet. Configure SCIM (see <a class="underline" href="/admin/sso">SSO</a>) to
    auto-provision from your IdP, or mint a team token via the admin CLI.
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
          <th class="px-4 py-3">Email</th>
          <th class="px-4 py-3">Name</th>
          <th class="px-4 py-3">Role</th>
          <th class="px-4 py-3">Joined</th>
          <th class="px-4 py-3 text-right">Actions</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-[var(--sp-border)]">
        {#each data.members as m (m.id)}
          <tr class={m.active ? '' : 'opacity-60'}>
            <td class="px-4 py-3 font-mono text-xs text-[var(--sp-fg)]">{m.email}</td>
            <td class="px-4 py-3 text-[var(--sp-fg)]">
              {m.display_name ?? '—'}
              {#if !m.active}
                <span class="ml-2 text-xs text-[var(--sp-muted-fg)]">(deactivated)</span>
              {/if}
            </td>
            <td class="px-4 py-3">
              <span class="rounded-full px-2 py-0.5 text-xs font-medium {badgeClass(m.role)}">
                {m.role}
              </span>
            </td>
            <td class="px-4 py-3 text-[var(--sp-muted-fg)]">{fmtDate(m.joined_at)}</td>
            <td class="px-4 py-3 text-right">
              <form method="POST" action="?/setRole" class="inline-flex items-center gap-2">
                <input type="hidden" name="id" value={m.id} />
                <select
                  name="role"
                  value={m.role}
                  class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] bg-[var(--sp-bg)] px-2 py-1 text-xs"
                >
                  {#each ROLES as r (r)}
                    <option value={r}>{r}</option>
                  {/each}
                </select>
                <button
                  type="submit"
                  class="rounded-[var(--sp-radius)] border border-[var(--sp-border)] px-3 py-1 text-xs hover:border-[var(--sp-primary)]"
                >
                  Set
                </button>
              </form>
              <form method="POST" action="?/remove" class="ml-2 inline-block">
                <input type="hidden" name="id" value={m.id} />
                <button
                  type="submit"
                  title="Remove from tenant"
                  class="inline-flex items-center gap-1 rounded-[var(--sp-radius)] border border-red-200 px-2 py-1 text-xs text-red-700 hover:bg-red-50"
                >
                  <Trash2 size="12" /> Remove
                </button>
              </form>
            </td>
          </tr>
        {/each}
      </tbody>
    </table>
  </div>
{/if}
