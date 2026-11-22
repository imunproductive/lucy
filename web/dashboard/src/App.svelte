<script lang="ts">
  import {app} from './lib/app.svelte.ts';
  import {router} from './lib/router.svelte.ts';
  import {theme} from './lib/theme.svelte.ts';
  import {clock, number} from './lib/format.ts';
  import {paths} from './lib/routes.ts';
  import type {PageName} from './lib/routes.ts';
  import Icon from './components/Icon.svelte';
  import type {IconName} from './components/Icon.svelte';
  import Logo from './components/Logo.svelte';
  import Toasts from './components/Toasts.svelte';
  import Profile from './views/Profile.svelte';
  import Events from './views/Events.svelte';
  import Boxes from './views/Boxes.svelte';
  import Collection from './views/Collection.svelte';
  import Server from './views/Server.svelte';
  import Matches from './views/Matches.svelte';
  import Match from './views/Match.svelte';
  import NotFound from './views/NotFound.svelte';

  interface NavItem {
    page: Exclude<PageName, 'match'>;
    label: string;
    icon: IconName;
  }

  const groups: {label: string; items: NavItem[]}[] = [
    {
      label: 'Account',
      items: [
        {page: 'profile', label: 'Profile', icon: 'profile'},
        {page: 'events', label: 'Events', icon: 'events'},
        {page: 'boxes', label: 'Loot boxes', icon: 'boxes'},
        {page: 'collection', label: 'Collection', icon: 'collection'},
      ],
    },
    {
      label: 'Server',
      items: [
        {page: 'server', label: 'Server', icon: 'server'},
        {page: 'matches', label: 'Live matches', icon: 'live'},
      ],
    },
  ];

  const titles: Record<PageName | 'not-found', string> = {
    profile: 'Profile',
    events: 'Events',
    boxes: 'Loot boxes',
    collection: 'Collection',
    server: 'Server',
    matches: 'Live matches',
    match: 'Match',
    'not-found': 'Page not found',
  };

  let menuOpen = $state(false);
  let switching = $state(false);

  const route = $derived(router.route);
  const active = $derived(route.name === 'match' ? 'matches' : route.name);
  const title = $derived(titles[route.name]);
  const server = $derived(app.state?.server);

  $effect(() => {
    document.title = `${title} · Lucy`;
  });

  // A page change closes the menu on small screens.
  $effect(() => {
    void router.path;
    menuOpen = false;
  });

  async function changeAccount(event: Event & {currentTarget: HTMLSelectElement}) {
    const select = event.currentTarget;
    const name = select.value;
    switching = true;
    try {
      await app.switchAccount(name);
    } catch (error) {
      select.value = app.account;
      app.fail(error);
    } finally {
      switching = false;
    }
  }
</script>

<a class="skip" href="#main">Skip to content</a>

<div class="shell" class:drawer={menuOpen}>
  <aside class="sidebar" aria-label="Navigation">
    <div class="brand">
      <a href="/" aria-label="Lucy: profile"><Logo /></a>
      <button class="icon-btn menu-close" type="button" aria-label="Close menu" onclick={() => (menuOpen = false)}>
        <Icon name="close" />
      </button>
    </div>

    <nav>
      {#each groups as group (group.label)}
        <div class="nav-group">
          <span class="nav-label">{group.label}</span>
          {#each group.items as item (item.page)}
            <a class="nav-link" class:active={active === item.page} aria-current={active === item.page ? 'page' : undefined} href={paths[item.page]}>
              <Icon name={item.icon} />
              <span>{item.label}</span>
              {#if item.page === 'boxes' && app.state}
                <span class="count numeric">{number(app.state.profile.loot_boxes_count)}</span>
              {:else if item.page === 'events' && app.state?.profile.events.length}
                <span class="pip" title="An event is on"></span>
              {/if}
            </a>
          {/each}
        </div>
      {/each}
    </nav>

    <div class="sidebar-foot">
      <span class="dot" class:on={app.connected === true} class:off={app.connected === false}></span>
      <div>
        <strong>{app.connected === false ? 'No connection' : app.connected ? 'Server is up' : 'Connecting…'}</strong>
        <span class="muted">{server ? `Lobby ${server.host}:${server.port}` : 'Local server'} · client 1.74</span>
      </div>
    </div>
  </aside>
  <button class="scrim" type="button" aria-label="Close menu" tabindex="-1" onclick={() => (menuOpen = false)}></button>

  <div class="workspace">
    <header class="topbar">
      <button class="icon-btn menu-open" type="button" aria-label="Open menu" onclick={() => (menuOpen = true)}>
        <Icon name="menu" />
      </button>
      <div class="crumbs">
        {#if route.name === 'match'}
          <a href={paths.matches}>Live matches</a><span class="sep">/</span>
        {/if}
        <strong>{title}</strong>
      </div>
      <div class="tools">
        <span class="sync muted" role="status">
          {#if app.connected === false}
            <span class="dot off"></span> No connection
          {:else if app.updated}
            Updated {clock(app.updated)}
          {:else}
            Connecting…
          {/if}
        </span>
        <button class="icon-btn" type="button" aria-label="Refresh" title="Refresh" disabled={app.refreshing} onclick={() => app.refresh()}>
          <Icon name="refresh" />
        </button>
        <button
          class="icon-btn"
          type="button"
          aria-label={theme.current === 'dark' ? 'Switch to light theme' : 'Switch to dark theme'}
          title={theme.current === 'dark' ? 'Light theme' : 'Dark theme'}
          onclick={() => theme.toggle()}
        >
          <Icon name={theme.current === 'dark' ? 'sun' : 'moon'} />
        </button>
        <label class="account">
          <span class="sr-only">Account</span>
          <Icon name="profile" size={16} />
          <select aria-label="Account" disabled={!app.state || switching} value={app.account} onchange={changeAccount}>
            {#if app.state}
              {#each app.state.accounts as row (row.name)}
                <option value={row.name}>{row.name}</option>
              {/each}
              {#if !app.state.accounts.some(row => row.name === app.account)}
                <option value={app.account}>{app.account}</option>
              {/if}
            {:else}
              <option value={app.account}>{app.account || 'Loading…'}</option>
            {/if}
          </select>
        </label>
      </div>
    </header>

    <main id="main" tabindex="-1">
      {#if app.error}
        <div class="notice error global-error" role="alert"><Icon name="alert" /> {app.error}</div>
      {/if}
      {#if route.name === 'matches'}
        <Matches />
      {:else if route.name === 'match'}
        <Match id={route.id} />
      {:else if route.name === 'not-found'}
        <NotFound />
      {:else if app.state}
        {#key app.account}
          {#if route.name === 'profile'}
            <Profile data={app.state} />
          {:else if route.name === 'events'}
            <Events data={app.state} />
          {:else if route.name === 'boxes'}
            <Boxes data={app.state} />
          {:else if route.name === 'collection'}
            <Collection data={app.state} />
          {:else if route.name === 'server'}
            <Server data={app.state} />
          {/if}
        {/key}
      {:else}
        <div class="loading" aria-busy="true" aria-label="Loading">
          <div class="bone wide"></div>
          <div class="bone"></div>
          <div class="bone tall"></div>
        </div>
      {/if}
    </main>
  </div>
</div>

<Toasts />

<style>
  .skip {
    position: absolute;
    left: 12px;
    top: -48px;
    z-index: 100;
    padding: 8px 12px;
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--accent-text);
  }

  .skip:focus {
    top: 12px;
  }

  .shell {
    display: grid;
    grid-template-columns: var(--sidebar) minmax(0, 1fr);
    min-height: 100vh;
  }

  .sidebar {
    position: sticky;
    top: 0;
    height: 100vh;
    display: flex;
    flex-direction: column;
    gap: var(--space-5);
    padding: var(--space-5) var(--space-4);
    border-right: 1px solid var(--border);
    background: var(--surface);
    overflow-y: auto;
  }

  .brand {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0 var(--space-2);
  }

  .brand a:hover {
    text-decoration: none;
  }

  nav {
    display: grid;
    gap: var(--space-5);
  }

  .nav-group {
    display: grid;
    gap: 2px;
  }

  .nav-label {
    padding: 0 var(--space-3) var(--space-2);
    font-size: 11px;
    font-weight: 650;
    letter-spacing: 0.07em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .nav-link {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: 8px var(--space-3);
    border-radius: var(--radius);
    color: var(--text-2);
    font-weight: 520;
  }

  .nav-link:hover {
    background: var(--surface-2);
    color: var(--text);
    text-decoration: none;
  }

  .nav-link.active {
    background: var(--accent-soft);
    color: var(--accent-soft-text);
  }

  .nav-link span:not(.count):not(.pip) {
    flex: 1;
  }

  .count {
    font-size: 12px;
    color: var(--muted);
  }

  .pip {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--accent);
  }

  .sidebar-foot {
    margin-top: auto;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface-2);
    font-size: 12.5px;
  }

  .sidebar-foot div {
    display: grid;
    min-width: 0;
  }

  .sidebar-foot .muted {
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .workspace {
    min-width: 0;
    display: flex;
    flex-direction: column;
  }

  .topbar {
    position: sticky;
    top: 0;
    z-index: 10;
    display: flex;
    align-items: center;
    gap: var(--space-3);
    min-height: 60px;
    padding: var(--space-2) var(--space-6);
    border-bottom: 1px solid var(--border);
    background: color-mix(in srgb, var(--bg) 86%, transparent);
    backdrop-filter: blur(10px);
  }

  .crumbs {
    display: flex;
    align-items: center;
    gap: var(--space-2);
    min-width: 0;
    font-size: 14.5px;
  }

  .crumbs a {
    color: var(--muted);
  }

  .sep {
    color: var(--border-strong);
  }

  .tools {
    display: flex;
    align-items: center;
    gap: var(--space-1);
    margin-left: auto;
  }

  .sync {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12.5px;
    margin-right: var(--space-2);
    white-space: nowrap;
  }

  .account {
    position: relative;
    display: flex;
    align-items: center;
    margin-left: var(--space-2);
    color: var(--muted);
  }

  .account :global(svg) {
    position: absolute;
    left: 10px;
    pointer-events: none;
  }

  .account select {
    min-width: 150px;
    max-width: 220px;
    padding-left: 32px;
    font-weight: 560;
  }

  main {
    width: 100%;
    max-width: calc(var(--content) + 2 * var(--space-6));
    padding: var(--space-6);
    outline: none;
  }

  .global-error {
    margin-bottom: var(--space-5);
    align-items: center;
  }

  .loading {
    display: grid;
    gap: var(--space-5);
  }

  .bone {
    height: 120px;
    border-radius: var(--radius-l);
    background: linear-gradient(90deg, var(--surface-2), var(--surface-3), var(--surface-2));
    background-size: 200% 100%;
    animation: shimmer 1.4s infinite linear;
  }

  .bone.wide {
    height: 160px;
  }

  .bone.tall {
    height: 320px;
  }

  @keyframes shimmer {
    to {
      background-position: -200% 0;
    }
  }

  .menu-open,
  .menu-close,
  .scrim {
    display: none;
  }

  @media (max-width: 900px) {
    .shell {
      grid-template-columns: minmax(0, 1fr);
    }

    .sidebar {
      position: fixed;
      inset: 0 auto 0 0;
      z-index: 40;
      width: min(var(--sidebar), 84vw);
      transform: translateX(-100%);
      transition: transform 0.2s var(--ease);
      box-shadow: var(--shadow-l);
    }

    .drawer .sidebar {
      transform: none;
    }

    .drawer .scrim {
      display: block;
      position: fixed;
      inset: 0;
      z-index: 30;
      border: 0;
      background: var(--scrim);
    }

    .menu-open,
    .menu-close {
      display: inline-grid;
    }

    .topbar {
      padding: var(--space-2) var(--space-4);
    }

    main {
      padding: var(--space-5) var(--space-4);
    }

    .sync {
      display: none;
    }
  }

  @media (max-width: 520px) {
    .crumbs {
      display: none;
    }

    .account select {
      min-width: 0;
      max-width: 150px;
    }
  }
</style>
