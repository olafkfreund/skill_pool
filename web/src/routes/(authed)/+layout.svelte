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
    BookOpen,
    BarChart3,
    FolderGit2,
    Layers,
    Package,
    UserCircle,
    ChevronDown,
    ChevronRight,
    LogOut,
  } from '@lucide/svelte';

  let { data, children } = $props();

  // Sidebar groups. `defaultOpen` controls the initial collapsed state.
  // Catalog + Insights stay open (frequent use). Settings + Help start
  // collapsed so the sidebar reads compact; clicking the header expands.
  // The expand state is mirrored to localStorage so a curator's choice
  // sticks across reloads.
  const groups = [
    {
      label: 'Catalog',
      defaultOpen: true,
      items: [
        { href: '/', icon: Library, label: 'Catalog' },
        { href: '/drafts', icon: Inbox, label: 'Drafts' },
        { href: '/admin/decay', icon: Skull, label: 'Graveyard' },
      ],
    },
    {
      label: 'Insights',
      defaultOpen: true,
      items: [{ href: '/admin/usage', icon: BarChart3, label: 'Usage' }],
    },
    {
      label: 'Settings',
      defaultOpen: false,
      items: [
        { href: '/profile', icon: UserCircle, label: 'Profile' },
        { href: '/admin/theme', icon: Palette, label: 'Theme' },
        { href: '/admin/members', icon: Users, label: 'Members' },
        { href: '/admin/sso', icon: ShieldCheck, label: 'SSO' },
        { href: '/admin/notifications', icon: Bell, label: 'Notifications' },
        { href: '/admin/projects', icon: FolderGit2, label: 'Projects' },
        { href: '/admin/plugins', icon: Package, label: 'Plugins' },
        { href: '/admin/stack-mappings', icon: Layers, label: 'Stack mappings' },
        { href: '/admin/domain', icon: Globe2, label: 'Domain' },
      ],
    },
    {
      label: 'Help',
      defaultOpen: false,
      items: [{ href: '/help', icon: BookOpen, label: 'Help & Docs' }],
    },
  ];

  const current = $derived(page.url.pathname);

  // Expand state per group label. Auto-expands if the current route lives
  // inside the group — so navigating directly to /admin/sso doesn't hide
  // its parent. The auto-expand wins over the persisted preference.
  function isInGroup(group: (typeof groups)[number]): boolean {
    return group.items.some((it) => it.href === current);
  }

  function initialOpen(): Record<string, boolean> {
    // SSR: just use defaultOpen (auto-expand for active group runs on
    // client too since it depends on current, which is fine).
    const state: Record<string, boolean> = {};
    for (const g of groups) state[g.label] = g.defaultOpen;
    return state;
  }

  let open = $state(initialOpen());

  // Hydrate from localStorage on mount; auto-expand active group.
  $effect(() => {
    if (typeof window === 'undefined') return;
    try {
      const raw = window.localStorage.getItem('sp:sidebar:open');
      if (raw) {
        const parsed = JSON.parse(raw) as Record<string, boolean>;
        for (const g of groups) {
          if (typeof parsed[g.label] === 'boolean') {
            open[g.label] = parsed[g.label];
          }
        }
      }
    } catch {
      // ignore corrupt JSON
    }
    for (const g of groups) {
      if (isInGroup(g)) open[g.label] = true;
    }
  });

  function toggleGroup(label: string) {
    open[label] = !open[label];
    if (typeof window !== 'undefined') {
      try {
        window.localStorage.setItem('sp:sidebar:open', JSON.stringify(open));
      } catch {
        // quota or disabled — fine, state lives only this session
      }
    }
  }
</script>

<div class="flex min-h-screen flex-col">
  <div class="flex flex-1">
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

      <nav class="flex-1 space-y-3 text-sm">
        {#each groups as group (group.label)}
          {@const isOpen = open[group.label]}
          {@const groupBadge =
            !isOpen && group.items.some((it) => it.href === '/drafts') && data.pendingDrafts > 0
              ? data.pendingDrafts
              : 0}
          <div>
            <button
              type="button"
              onclick={() => toggleGroup(group.label)}
              class="flex w-full items-center gap-1 rounded-md px-3 py-1 text-[10px] font-semibold tracking-wider text-[var(--sp-muted-fg)] uppercase transition-colors hover:bg-[var(--sp-bg)] hover:text-[var(--sp-fg)]"
              aria-expanded={isOpen}
              aria-controls={`group-${group.label}`}
            >
              {#if isOpen}
                <ChevronDown size="11" />
              {:else}
                <ChevronRight size="11" />
              {/if}
              <span class="flex-1 text-left">{group.label}</span>
              {#if groupBadge > 0}
                <span
                  class="rounded-full px-1.5 py-0.5 text-[9px] leading-none normal-case"
                  style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                  title={`${groupBadge} pending`}
                >
                  {groupBadge > 99 ? '99+' : groupBadge}
                </span>
              {/if}
            </button>
            {#if isOpen}
              <div id={`group-${group.label}`} class="mt-1 space-y-1">
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
                        class="rounded-full px-1.5 py-0.5 text-[10px] leading-none font-medium"
                        style="background: var(--sp-primary); color: var(--sp-primary-fg);"
                        title={`${badge} pending draft${badge === 1 ? '' : 's'}`}
                      >
                        {badge > 99 ? '99+' : badge}
                      </span>
                    {/if}
                  </a>
                {/each}
              </div>
            {/if}
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

  {#if data.theme.footerBranding}
    <footer
      class="border-t border-[var(--sp-border)] py-3 text-center text-xs text-[var(--sp-muted-fg)]"
    >
      Powered by <a
        href="https://github.com/anthropics/skill-pool"
        class="underline hover:text-[var(--sp-fg)]"
        target="_blank"
        rel="noopener noreferrer">skill-pool</a
      >
    </footer>
  {/if}
</div>
