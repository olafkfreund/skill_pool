<script lang="ts">
  import { page } from '$app/state';
  import {
    Library,
    ShieldCheck,
    Palette,
    Users,
    Globe2,
    Inbox,
    Bell,
    Skull,
    LogOut,
  } from '@lucide/svelte';

  let { data, children } = $props();

  // Two grouped sections. Catalog group = day-to-day curation surfaces;
  // Settings group = tenant-wide config. Order within each group
  // reflects how often the curator uses them.
  const groups = [
    {
      label: 'Catalog',
      items: [
        { href: '/', icon: Library, label: 'Catalog' },
        { href: '/drafts', icon: Inbox, label: 'Drafts' },
        { href: '/admin/decay', icon: Skull, label: 'Graveyard' },
      ],
    },
    {
      label: 'Settings',
      items: [
        { href: '/admin/theme', icon: Palette, label: 'Theme' },
        { href: '/admin/members', icon: Users, label: 'Members' },
        { href: '/admin/sso', icon: ShieldCheck, label: 'SSO' },
        { href: '/admin/notifications', icon: Bell, label: 'Notifications' },
        { href: '/admin/domain', icon: Globe2, label: 'Domain' },
      ],
    },
  ];

  const current = $derived(page.url.pathname);
</script>

<div class="flex min-h-screen">
  <aside
    class="hidden w-60 shrink-0 border-r border-[var(--sp-border)] bg-[var(--sp-muted)] px-4 py-6 md:flex md:flex-col"
  >
    <div class="mb-8 flex items-center gap-2 px-2">
      <div
        class="grid h-8 w-8 place-items-center rounded-md font-bold"
        style="background: var(--sp-primary); color: var(--sp-primary-fg);"
      >
        {data.theme.brandName.charAt(0).toUpperCase()}
      </div>
      <div class="text-sm">
        <div class="font-semibold">{data.theme.brandName}</div>
        <div class="text-xs text-[var(--sp-muted-fg)]">{data.tenant.slug}</div>
      </div>
    </div>

    <nav class="flex-1 space-y-5 text-sm">
      {#each groups as group (group.label)}
        <div>
          <div
            class="mb-1 px-3 text-[10px] font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase"
          >
            {group.label}
          </div>
          <div class="space-y-1">
            {#each group.items as item (item.href)}
              {@const Icon = item.icon}
              {@const active = current === item.href}
              {@const badge =
                item.href === '/drafts' && data.pendingDrafts > 0 ? data.pendingDrafts : 0}
              <a
                href={item.href}
                class="flex items-center gap-3 rounded-md px-3 py-2 transition-colors {active
                  ? 'bg-[var(--sp-bg)] font-medium text-[var(--sp-fg)]'
                  : 'text-[var(--sp-muted-fg)] hover:bg-[var(--sp-bg)] hover:text-[var(--sp-fg)]'}"
              >
                <Icon size="16" />
                <span class="flex-1">{item.label}</span>
                {#if badge > 0}
                  <span
                    class="rounded-full px-1.5 py-0.5 text-[10px] font-medium leading-none"
                    style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                    title={`${badge} pending draft${badge === 1 ? '' : 's'}`}
                  >
                    {badge > 99 ? '99+' : badge}
                  </span>
                {/if}
              </a>
            {/each}
          </div>
        </div>
      {/each}
    </nav>

    <form method="POST" action="/logout" class="mt-4">
      <button
        type="submit"
        class="flex w-full items-center gap-3 rounded-md px-3 py-2 text-sm text-[var(--sp-muted-fg)] hover:text-[var(--sp-fg)]"
      >
        <LogOut size="16" />
        Sign out
      </button>
    </form>
  </aside>

  <main class="flex-1 px-6 py-8 md:px-10">
    {@render children?.()}
  </main>
</div>
