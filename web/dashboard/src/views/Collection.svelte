<script lang="ts">
  import * as api from '../lib/api.ts';
  import {app} from '../lib/app.svelte.ts';
  import {currencies, initials, itemTypes, number, price} from '../lib/format.ts';
  import type {CollectionItem, CollectionKind, CollectionPage, CollectionQuery, Currency, State} from '../lib/types.ts';
  import Coin from '../components/Coin.svelte';
  import Dialog from '../components/Dialog.svelte';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';

  let {data}: {data: State} = $props();

  const kinds: [CollectionKind, string][] = [
    ['all', 'All items'],
    ['skins', 'Skins'],
    ['weapons', 'Weapons'],
    ['icons', 'Player icons'],
    ['sprays', 'Sprays'],
    ['emotes', 'Emotes'],
    ['poses', 'Victory poses'],
    ['voicelines', 'Voice lines'],
    ['intros', 'Highlight intros'],
    ['frames', 'Portrait frames'],
  ];
  const coins: Currency[] = ['credits', 'comp_points', 'league_tokens'];

  let kind = $state<CollectionKind>('all');
  let typed = $state('');
  let search = $state('');
  let hero = $state('');
  let currency = $state<Currency | ''>('');
  let owl = $state(false);
  let page = $state(1);

  let result = $state<CollectionPage | null>(null);
  let loading = $state(false);
  let error = $state('');
  let request = 0;
  let timer: ReturnType<typeof setTimeout> | undefined;

  const profile = $derived(data.profile);

  // The search waits for a pause in typing.
  function onSearch() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      search = typed.trim();
      page = 1;
    }, 300);
  }

  function filtersChanged() {
    page = 1;
  }

  function resetFilters() {
    kind = 'all';
    typed = search = hero = currency = '';
    owl = false;
    page = 1;
  }

  async function load(query: CollectionQuery) {
    const id = ++request;
    loading = true;
    try {
      const next = await api.getCollection(query);
      if (id !== request) return;
      result = next;
      error = '';
    } catch (failure) {
      if (id === request) error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      if (id === request) loading = false;
    }
  }

  // Reload on each filter change and each refresh of the state (an item
  // bought or unlocked changes the page).
  $effect(() => {
    void app.revision;
    void load({account: app.account, kind, q: search, hero, currency, owl, page});
  });

  $effect(() => () => clearTimeout(timer));

  /** An item action; the profile (frame, counts) changes too, so the state is read again. */
  async function act(run: () => Promise<unknown>, done: string) {
    error = '';
    try {
      await run();
      app.toast(done);
      await app.refresh();
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
      app.fail(failure);
    }
  }

  const setFrame = (item: CollectionItem, guid: string | null) =>
    act(() => api.setFrame(app.account, guid), guid ? `Frame: ${item.name}` : 'Back to the level frame');
  const grant = (item: CollectionItem, revoke: boolean) =>
    act(() => api.grantItem(app.account, item.guid, revoke), revoke ? `Removed ${item.name}` : `Granted ${item.name}`);

  // Buying, with a confirmation that shows the balance after.
  let buying = $state<CollectionItem | null>(null);
  let buyOpen = $state(false);
  let buyBusy = $state(false);
  let buyError = $state('');
  const balance = $derived(buying?.currency ? Number(profile[buying.currency]) || 0 : 0);
  const affordable = $derived(!!buying && balance >= Number(buying.price));

  function openPurchase(item: CollectionItem) {
    buying = item;
    buyError = '';
    buyOpen = true;
  }

  async function confirmPurchase() {
    if (!buying || buyBusy || !affordable) return;
    const item = buying;
    buyBusy = true;
    buyError = '';
    try {
      const result = await api.purchase(app.account, item.guid);
      buyOpen = false;
      app.toast(`Bought ${item.name} for ${price(result.receipt.price, result.receipt.currency)}.`);
      await app.refresh();
    } catch (failure) {
      buyError = failure instanceof Error ? failure.message : String(failure);
    } finally {
      buyBusy = false;
    }
  }

  let broken = $state(new Set<string>());
</script>

<div class="page">
  <header class="page-header">
    <div>
      <h1>Collection</h1>
      <p>Buy or unlock any item, including OWL and event items you can no longer get, or pick a portrait frame.</p>
    </div>
    <div class="balances" aria-label="Balances">
      {#each coins as coin (coin)}
        <span class="balance" title={currencies[coin].label}><Coin currency={coin} size={20} /><b class="numeric">{number(profile[coin])}</b></span>
      {/each}
    </div>
  </header>

  <form class="filters card" role="search" onsubmit={event => event.preventDefault()}>
    <label class="search">
      <Icon name="search" />
      <input type="search" placeholder="Search items or heroes" aria-label="Search items" bind:value={typed} oninput={onSearch} />
    </label>
    <select aria-label="Type" bind:value={kind} onchange={filtersChanged}>
      {#each kinds as [value, label] (value)}<option {value}>{label}</option>{/each}
    </select>
    <select aria-label="Hero" bind:value={hero} onchange={filtersChanged}>
      <option value="">All heroes</option>
      {#each data.catalogs.heroes as item (item.guid)}<option value={item.name}>{item.name}</option>{/each}
    </select>
    <select aria-label="Currency" bind:value={currency} onchange={filtersChanged}>
      <option value="">All currencies</option>
      {#each coins as coin (coin)}<option value={coin}>{currencies[coin].label}</option>{/each}
    </select>
    <label class="owl"><input type="checkbox" bind:checked={owl} onchange={filtersChanged} /> OWL only</label>
  </form>

  <div class="result-line">
    <span class="muted">{result ? `${number(result.total)} items` : 'Loading…'}</span>
    {#if loading && result}<span class="muted" role="status">Updating…</span>{/if}
  </div>
  {#if error}<div class="notice error" role="alert"><Icon name="alert" /> {error}</div>{/if}

  {#if result && result.items.length}
    <div class="items" aria-busy={loading}>
      {#each result.items as item (item.guid)}
        <article class="item rarity-{item.rarity.toLowerCase()}">
          <div class="art">
            <span class="type">{itemTypes[item.type] ?? item.type ?? 'Item'}</span>
            {#if item.preview && !broken.has(item.guid)}
              <img src={item.preview} alt="" loading="lazy" onerror={() => (broken = new Set(broken).add(item.guid))} />
            {:else}
              <span class="monogram">{initials(item.hero || item.name)}</span>
            {/if}
            {#if item.owned || item.in_use}
              <span class="owned"><Icon name="check" size={13} /> {item.frame ? 'In use' : 'Owned'}</span>
            {/if}
          </div>
          <div class="copy">
            <span class="hero-name">{item.frame ? 'Portrait frame' : item.hero || 'Generic item'}</span>
            <h3>{item.name}</h3>
            <span class="rarity">{item.rarity}</span>
          </div>
          <div class="item-actions">
            {#if item.frame}
              {#if item.chosen}
                <button class="btn small" type="button" onclick={() => setFrame(item, null)}>Use level frame</button>
              {:else}
                <button class="btn small" class:primary={!item.in_use} type="button" disabled={item.in_use} onclick={() => setFrame(item, item.guid)}>
                  {item.in_use ? 'In use' : 'Use'}
                </button>
              {/if}
            {:else if item.owned}
              {#if item.removable}
                <button class="btn small danger" type="button" onclick={() => grant(item, true)}>Remove</button>
              {:else}
                <span class="muted small">In your collection</span>
              {/if}
            {:else}
              {#if item.purchasable && item.currency}
                <span class="price" aria-label={price(item.price, item.currency)}><Coin currency={item.currency} size={16} />{number(item.price)}</span>
                <button class="btn small primary" type="button" aria-label="Buy {item.name} for {price(item.price, item.currency)}" onclick={() => openPurchase(item)}>Buy</button>
              {/if}
              <button class="btn small" class:primary={!item.purchasable} type="button" title="Add it for free" onclick={() => grant(item, false)}>Unlock</button>
            {/if}
          </div>
        </article>
      {/each}
    </div>
  {:else if result}
    <div class="card">
      <EmptyState icon="search" title="No items found" text="Change the search or the filters.">
        <button class="btn" type="button" onclick={resetFilters}>Reset filters</button>
      </EmptyState>
    </div>
  {/if}

  {#if result && result.pages > 1}
    <nav class="pager" aria-label="Pages">
      <button class="btn" type="button" disabled={loading || page <= 1} onclick={() => (page -= 1)}><Icon name="left" size={16} /> Back</button>
      <span class="muted numeric">Page {number(result.page)} of {number(result.pages)}</span>
      <button class="btn" type="button" disabled={loading || page >= result.pages} onclick={() => (page += 1)}>Next <Icon name="right" size={16} /></button>
    </nav>
  {/if}
</div>

<Dialog bind:open={buyOpen} title={buying?.name ?? ''} kicker="Buy" busy={buyBusy}>
  {#if buying}
    <p class="muted">{buying.hero || 'Generic item'} · {itemTypes[buying.type] ?? buying.type}</p>
    <dl class="receipt">
      <div><dt>Account</dt><dd>{app.account}</dd></div>
      <div><dt>Price</dt><dd>{price(buying.price, buying.currency)}</dd></div>
      <div><dt>Balance after</dt><dd>{price(balance - Number(buying.price), buying.currency)}</dd></div>
    </dl>
    {#if !affordable}
      <p class="error-text" role="alert">Not enough funds. Balance: {price(balance, buying.currency)}.</p>
    {/if}
    {#if buyError}<p class="error-text" role="alert">{buyError}</p>{/if}
  {/if}
  {#snippet actions()}
    <button class="btn" type="button" disabled={buyBusy} onclick={() => (buyOpen = false)}>Cancel</button>
    <button class="btn primary" type="button" disabled={buyBusy || !affordable} onclick={confirmPurchase}>{buyBusy ? 'Buying…' : 'Buy item'}</button>
  {/snippet}
</Dialog>

<style>
  .balances {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
  }

  .balance {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: 6px 12px 6px 8px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface);
    font-size: 13.5px;
  }

  .filters {
    display: grid;
    grid-template-columns: minmax(220px, 2fr) repeat(3, minmax(140px, 1fr)) auto;
    gap: var(--space-3);
    align-items: center;
    padding: var(--space-3);
  }

  @media (max-width: 980px) {
    .filters {
      grid-template-columns: 1fr 1fr;
    }

    .search {
      grid-column: 1 / -1;
    }
  }

  .search {
    position: relative;
    display: flex;
    align-items: center;
    color: var(--muted);
  }

  .search :global(svg) {
    position: absolute;
    left: 11px;
    pointer-events: none;
  }

  .search input {
    padding-left: 36px;
  }

  .owl {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    padding: 0 var(--space-2);
    white-space: nowrap;
    font-weight: 540;
    cursor: pointer;
  }

  .owl input {
    accent-color: var(--accent);
    width: 16px;
    height: 16px;
  }

  .result-line {
    display: flex;
    justify-content: space-between;
    margin-bottom: calc(-1 * var(--space-3));
    font-size: 13px;
  }

  .items {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(200px, 1fr));
    gap: var(--space-4);
    transition: opacity 0.15s;
  }

  @media (max-width: 520px) {
    .items {
      grid-template-columns: repeat(2, minmax(0, 1fr));
      gap: var(--space-3);
    }

    .copy,
    .item-actions {
      padding-left: var(--space-3);
      padding-right: var(--space-3);
    }
  }

  .items[aria-busy='true'] {
    opacity: 0.7;
  }

  .item {
    --rarity: var(--rarity-common);
    display: flex;
    flex-direction: column;
    overflow: hidden;
    border: 1px solid var(--border);
    border-radius: var(--radius-l);
    background: var(--surface);
    box-shadow: var(--shadow);
  }

  .rarity-rare {
    --rarity: var(--rarity-rare);
  }

  .rarity-epic {
    --rarity: var(--rarity-epic);
  }

  .rarity-legendary {
    --rarity: var(--rarity-legendary);
  }

  .art {
    position: relative;
    display: grid;
    place-items: center;
    aspect-ratio: 4 / 3;
    background:
      radial-gradient(circle at 50% 120%, color-mix(in srgb, var(--rarity) 35%, transparent), transparent 70%),
      var(--surface-2);
    border-bottom: 2px solid var(--rarity);
  }

  .art img {
    width: 100%;
    height: 100%;
    object-fit: contain;
  }

  .monogram {
    font-size: 30px;
    font-weight: 750;
    letter-spacing: -0.02em;
    color: color-mix(in srgb, var(--rarity) 70%, var(--text));
  }

  .type,
  .owned {
    position: absolute;
    top: 8px;
    padding: 2px 8px;
    border-radius: 999px;
    font-size: 11px;
    font-weight: 600;
    background: color-mix(in srgb, var(--surface) 88%, transparent);
    backdrop-filter: blur(4px);
  }

  .type {
    left: 8px;
    color: var(--text-2);
  }

  .owned {
    right: 8px;
    display: inline-flex;
    align-items: center;
    gap: 3px;
    color: var(--success);
  }

  .copy {
    display: grid;
    gap: 2px;
    padding: var(--space-3) var(--space-4) 0;
    flex: 1;
  }

  .hero-name {
    font-size: 12px;
    color: var(--muted);
  }

  .copy h3 {
    overflow-wrap: anywhere;
  }

  .rarity {
    font-size: 12px;
    font-weight: 600;
    color: var(--rarity);
  }

  .item-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: flex-end;
    gap: var(--space-2);
    padding: var(--space-3) var(--space-4) var(--space-4);
  }

  .price {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    margin-right: auto;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
  }

  .small {
    font-size: 12.5px;
  }

  .pager {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: var(--space-4);
  }

  .receipt {
    display: grid;
    gap: var(--space-2);
    margin: 0;
    padding: var(--space-3) var(--space-4);
    border-radius: var(--radius);
    background: var(--surface-2);
  }

  .receipt div {
    display: flex;
    justify-content: space-between;
  }

  .receipt dt {
    color: var(--muted);
  }

  .receipt dd {
    margin: 0;
    font-weight: 600;
  }
</style>
