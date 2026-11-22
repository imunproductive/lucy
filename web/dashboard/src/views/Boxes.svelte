<script lang="ts">
  import * as api from '../lib/api.ts';
  import {app} from '../lib/app.svelte.ts';
  import {number} from '../lib/format.ts';
  import type {State} from '../lib/types.ts';
  import BoxArt from '../components/BoxArt.svelte';
  import Dialog from '../components/Dialog.svelte';
  import Icon from '../components/Icon.svelte';

  let {data}: {data: State} = $props();

  const quick = [1, 5, 10, 25, 100];
  let picked = $state<number | null>(null);
  let count = $state(5);
  let giving = $state(false);
  let opening = $state(false);
  let confirmOpen = $state(false);
  let error = $state('');

  const types = $derived(data.catalogs.box_types);
  const selected = $derived(types.find(box => box.id === picked) ?? types[0] ?? null);
  const total = $derived(data.profile.loot_boxes_count);

  function stock(type: number): number {
    return data.profile.box_counts.find(row => row.type === type)?.count ?? 0;
  }

  function setCount(value: number) {
    count = Math.max(1, Math.min(100, Math.floor(value) || 1));
  }

  async function give(submit: SubmitEvent) {
    submit.preventDefault();
    const form = submit.currentTarget as HTMLFormElement;
    if (giving || !selected || !form.reportValidity()) return;
    const box = selected;
    giving = true;
    error = '';
    const release = app.hold();
    try {
      const result = await api.addBoxes(app.account, box.id, count);
      await app.refresh();
      app.toast(`Added ${number(result.added)} ${box.label || box.name} boxes. Total: ${number(result.total_boxes)}.`);
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      giving = false;
      release();
    }
  }

  async function openAll() {
    opening = true;
    error = '';
    try {
      const result = await api.openAllBoxes(app.account);
      confirmOpen = false;
      await app.refresh();
      app.toast(`Opened ${number(result.opened)} boxes, ${number(result.new_items)} new items.`);
    } catch (failure) {
      confirmOpen = false;
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      opening = false;
    }
  }
</script>

<div class="page">
  <header class="page-header">
    <div>
      <h1>Loot boxes</h1>
      <p>Box types and how many <strong>{app.account}</strong> has.</p>
    </div>
    <div class="stat total"><span>In stock</span><strong>{number(total)}</strong></div>
  </header>

  <div class="columns">
    <div class="box-grid">
      {#each types as box (box.id)}
        <button class="tile box" type="button" aria-pressed={selected?.id === box.id} onclick={() => ((picked = box.id), (error = ''))}>
          <BoxArt {box} />
          <strong>{box.label || box.name}</strong>
          <span class="stock"><span class="muted">In stock</span><b class="numeric">{number(stock(box.id))}</b></span>
        </button>
      {:else}
        <p class="muted">No box types.</p>
      {/each}
    </div>

    <aside class="card give">
      <div class="card-header"><h2>Give boxes</h2></div>
      {#if selected}
        <div class="selected-art"><BoxArt box={selected} /></div>
        <div class="selected-name">
          <h3>{selected.label || selected.name}</h3>
          <span class="muted">In stock: {number(stock(selected.id))}</span>
        </div>
      {/if}
      <form onsubmit={give}>
        <label class="field">
          <span>Quantity</span>
          <div class="stepper">
            <button class="icon-btn" type="button" aria-label="Fewer" disabled={giving} onclick={() => setCount(count - 1)}><Icon name="minus" /></button>
            <input type="number" min="1" max="100" required aria-label="Quantity" bind:value={count} disabled={giving} />
            <button class="icon-btn" type="button" aria-label="More" disabled={giving} onclick={() => setCount(count + 1)}><Icon name="plus" /></button>
          </div>
        </label>
        <div class="quick" role="group" aria-label="Quantity presets">
          {#each quick as value (value)}
            <button class="btn small" class:active={count === value} type="button" disabled={giving} onclick={() => setCount(value)}>{value}</button>
          {/each}
        </div>
        {#if error}<p class="error-text" role="alert">{error}</p>{/if}
        <button class="btn primary wide" type="submit" disabled={giving || !selected}><Icon name="gift" size={16} /> {giving ? 'Adding…' : 'Give boxes'}</button>
      </form>
      <button class="btn danger wide" type="button" disabled={opening || !total} onclick={() => (confirmOpen = true)}>Open all boxes</button>
    </aside>
  </div>
</div>

<Dialog bind:open={confirmOpen} title="Open every box?" kicker="Loot boxes" busy={opening}>
  <p class="muted">
    All {number(total)} boxes of {app.account} open at once, like the move to Overwatch 2 (the xQc incident). The game shows how many on the main menu.
  </p>
  {#snippet actions()}
    <button class="btn" type="button" disabled={opening} onclick={() => (confirmOpen = false)}>Cancel</button>
    <button class="btn primary" type="button" disabled={opening} onclick={openAll}>{opening ? 'Opening…' : 'Open all'}</button>
  {/snippet}
</Dialog>

<style>
  .total {
    padding: var(--space-2) var(--space-4);
    text-align: right;
  }

  .box-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(160px, 1fr));
    gap: var(--space-3);
  }

  .box {
    gap: var(--space-2);
  }

  .stock {
    display: flex;
    justify-content: space-between;
    font-size: 12.5px;
  }

  .give {
    position: sticky;
    top: 84px;
    display: grid;
    gap: var(--space-4);
  }

  .give .card-header {
    margin: 0;
  }

  .selected-art {
    width: 140px;
    margin: 0 auto;
  }

  .selected-name {
    text-align: center;
    display: grid;
  }

  form {
    display: grid;
    gap: var(--space-3);
  }

  .stepper {
    display: grid;
    grid-template-columns: auto 1fr auto;
    gap: var(--space-2);
    align-items: center;
  }

  .stepper input {
    text-align: center;
  }

  .stepper .icon-btn {
    border: 1px solid var(--border-strong);
  }

  .quick {
    display: grid;
    grid-template-columns: repeat(5, 1fr);
    gap: var(--space-2);
  }

  .quick .btn {
    padding: 0;
  }

  .quick .active {
    border-color: var(--accent);
    background: var(--accent-soft);
    color: var(--accent-soft-text);
  }
</style>
