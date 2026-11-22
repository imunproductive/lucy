<script lang="ts">
  import {untrack} from 'svelte';
  import * as api from '../lib/api.ts';
  import {app} from '../lib/app.svelte.ts';
  import {changes, draftOf, rebase} from '../lib/draft.ts';
  import {itemTypes} from '../lib/format.ts';
  import type {EventInfo, Profile, State} from '../lib/types.ts';
  import Toggle from '../components/Toggle.svelte';

  let {data}: {data: State} = $props();

  const categories: Record<string, string> = {
    default: 'Standard lobby',
    seasonal: 'Seasonal event',
    special: 'Special look',
    challenge: 'Hero challenge',
    league: 'Overwatch League',
  };
  const noEvent: EventInfo = {id: '', label: 'No event', description: 'Standard lobby look', category: 'default', scene_status: '', scene_note: ''};

  const profile = $derived(data.profile);
  const events = $derived([noEvent, ...data.catalogs.events]);
  const current = $derived(profile.events[0] ?? '');

  // The lobby event: the picked one until it is applied, else the profile's.
  let picked = $state<string | null>(null);
  let eventForAll = $state(false);
  let applying = $state(false);
  let eventError = $state('');
  const selectedId = $derived(picked ?? current);
  const selected = $derived(events.find(item => item.id === selectedId));
  const eventDirty = $derived(picked !== null && picked !== current);

  function pick(id: string) {
    picked = id;
    eventError = '';
  }

  async function applyEvent() {
    if (applying || !(eventDirty || eventForAll)) return;
    const account = app.account;
    const id = selectedId;
    const label = events.find(item => item.id === id)?.label ?? 'none';
    const list = id ? [id] : [];
    applying = true;
    eventError = '';
    const release = app.hold();
    try {
      const result = eventForAll ? await api.applyToAll({events: list}) : await api.updateProfile(account, {events: list});
      picked = null;
      await app.refresh();
      app.toast('accounts' in result ? `Event set for all players (${result.accounts}): ${label}.` : `Event set: ${label}.`);
    } catch (error) {
      eventError = error instanceof Error ? error.message : String(error);
    } finally {
      applying = false;
      release();
    }
  }

  // The hero challenge form.
  const challengeFields = ['challenge', 'challenge_wins'] as const satisfies readonly (keyof Profile)[];
  let base: Profile = untrack(() => data.profile);
  let draft = $state(draftOf(base, challengeFields));
  let challengeForAll = $state(false);
  let saving = $state(false);
  let challengeError = $state('');

  $effect.pre(() => {
    const next = profile;
    if (next === base) return;
    rebase(draft, base, next, challengeFields);
    base = next;
  });

  const challengeDirty = $derived(changes(draft, profile, challengeFields));
  const canSave = $derived(challengeForAll || Object.keys(challengeDirty).length > 0);
  const challenge = $derived(data.catalogs.challenges.find(item => item.id === draft.challenge));

  async function saveChallenge(submit: SubmitEvent) {
    submit.preventDefault();
    const form = submit.currentTarget as HTMLFormElement;
    if (saving || !canSave || !form.reportValidity()) return;
    const account = app.account;
    saving = true;
    challengeError = '';
    const release = app.hold();
    try {
      // For all players: the challenge alone; the wins stay each player's own.
      if (challengeForAll) {
        const result = await api.applyToAll({challenge: draft.challenge});
        await app.refresh();
        app.toast(`Challenge set for all players (${result.accounts}).`);
      } else {
        await api.updateProfile(account, challengeDirty);
        await app.refresh();
        app.toast('Challenge saved.');
      }
    } catch (error) {
      challengeError = error instanceof Error ? error.message : String(error);
    } finally {
      saving = false;
      release();
    }
  }
</script>

<div class="page">
  <header class="page-header">
    <div>
      <h1>Events</h1>
      <p>The lobby's look and the hero challenge of <strong>{app.account}</strong>.</p>
    </div>
    <span class="badge accent">Now: {events.find(item => item.id === current)?.label ?? (current || 'no event')}</span>
  </header>

  <div class="columns">
    <section aria-labelledby="lobby-event">
      <h2 id="lobby-event" class="section-label">Lobby event</h2>
      <div class="event-grid">
        {#each events as event (event.id)}
          <button class="tile" type="button" aria-pressed={selectedId === event.id} onclick={() => pick(event.id)}>
            <span class="tile-top">
              <strong>{event.label}</strong>
              {#if event.id === current}<span class="badge success">On</span>{/if}
            </span>
            <span class="muted description">{event.description || 'Lobby look'}</span>
            <span class="category">{categories[event.category] ?? 'Game event'}</span>
          </button>
        {/each}
      </div>
    </section>

    <aside class="card detail">
      <span class="section-label">Selected</span>
      <h2>{selected?.label ?? 'Unknown event'}</h2>
      <p class="muted">{selected?.description || 'Pick an event from the list.'}</p>
      {#if !selectedId}
        <span class="badge">Standard look</span>
      {:else if selected?.scene_status === 'verified'}
        <span class="badge success">Tested</span>
      {:else if selected?.scene_status === 'limited'}
        <span class="badge warning">Partly works</span>
      {:else}
        <span class="badge warning">Not tested</span>
      {/if}
      <p class="hint">{selected?.scene_note || (selectedId ? 'Check it in the game.' : 'Default lobby.')}</p>
      <div class="detail-foot">
        <Toggle bind:checked={eventForAll} label="For all players" description="Every account, and new ones." />
        {#if eventError}<p class="error-text" role="alert">{eventError}</p>{/if}
        <button class="btn primary wide" type="button" disabled={applying || !(eventDirty || eventForAll)} onclick={applyEvent}>
          {applying ? 'Applying…' : eventForAll ? 'Apply to all players' : 'Apply event'}
        </button>
        <p class="hint">Some scenes do not load in the 1.74 client.</p>
      </div>
    </aside>
  </div>

  <form class="card" onsubmit={saveChallenge} aria-busy={saving}>
    <div class="card-header">
      <div>
        <h2>Hero challenge</h2>
        <p>Pick a challenge and set the wins.</p>
      </div>
    </div>
    <div class="challenge-row">
      <label class="field">
        <span>Challenge</span>
        <select bind:value={draft.challenge} disabled={saving}>
          <option value="">No challenge</option>
          {#each data.catalogs.challenges as item (item.id)}<option value={item.id}>{item.title}</option>{/each}
        </select>
      </label>
      <label class="field wins">
        <span>Wins</span>
        <input type="number" min="0" max="1000000" required bind:value={draft.challenge_wins} disabled={saving || challengeForAll} />
      </label>
    </div>
    <div class="rewards">
      {#if !challenge}
        <span class="muted">No challenge.</span>
      {:else if !challenge.rewards.length}
        <span class="muted">No rewards listed.</span>
      {:else}
        {#each challenge.rewards as reward, index (index)}
          <span class="reward">{reward.name || 'Item'} <small>{itemTypes[reward.type] ?? reward.type}</small></span>
        {/each}
      {/if}
    </div>
    <div class="card-footer">
      <div class="grow">
        <Toggle bind:checked={challengeForAll} label="For all players" description="Every account, and new ones. Wins stay each player's own." />
      </div>
      {#if challengeError}<span class="error-text" role="alert">{challengeError}</span>{/if}
      <button class="btn primary" type="submit" disabled={saving || !canSave}>{saving ? 'Saving…' : 'Save challenge'}</button>
    </div>
  </form>
</div>

<style>
  .event-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(210px, 1fr));
    gap: var(--space-3);
  }

  .tile-top {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-2);
  }

  .description {
    font-size: 13px;
  }

  .category {
    margin-top: var(--space-2);
    font-size: 11.5px;
    font-weight: 600;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--accent);
  }

  .detail {
    position: sticky;
    top: 84px;
    display: grid;
    gap: var(--space-3);
    justify-items: start;
  }

  .detail .section-label {
    margin: 0;
  }

  .detail-foot {
    display: grid;
    gap: var(--space-3);
    width: 100%;
    margin-top: var(--space-2);
    padding-top: var(--space-4);
    border-top: 1px solid var(--border);
  }

  .challenge-row {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 160px;
    gap: var(--space-4);
  }

  @media (max-width: 560px) {
    .challenge-row {
      grid-template-columns: 1fr;
    }
  }

  .rewards {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-2);
    margin-top: var(--space-4);
  }

  .reward {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 999px;
    background: var(--surface-2);
    font-size: 13px;
    font-weight: 540;
  }

  .reward small {
    color: var(--muted);
  }
</style>
