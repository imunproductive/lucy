<script lang="ts">
  import {app} from '../lib/app.svelte.ts';
  import {duration, number} from '../lib/format.ts';
  import * as live from '../lib/live.ts';
  import type {Broadcast, LiveSnapshot} from '../lib/live.ts';
  import {matchPath, paths} from '../lib/routes.ts';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';

  // Every running match, polled from /api/live (stage 3). Until the server
  // has the route, the page says so.
  let snapshot = $state<LiveSnapshot | null>(null);
  let available = $state<boolean | null>(null);
  let error = $state('');

  $effect(() => {
    const poll = live.pollLive({
      update: next => {
        snapshot = next;
        available = true;
        error = '';
      },
      unavailable: () => (available = false),
      failed: failure => (error = failure instanceof Error ? failure.message : String(failure)),
    });
    return () => poll.stop();
  });

  let message = $state('');
  let scope = $state<Broadcast['scope']>('all');
  let sending = $state(false);

  async function send(submit: SubmitEvent) {
    submit.preventDefault();
    if (!message.trim() || sending) return;
    sending = true;
    try {
      await live.broadcast({text: message.trim(), scope});
      app.toast('Message sent.');
      message = '';
    } catch (failure) {
      app.fail(failure);
    } finally {
      sending = false;
    }
  }
</script>

<div class="page">
  <header class="page-header">
    <div>
      <h1>Live matches</h1>
      <p>Every running match: mode, map, players, time and state, updated live.</p>
    </div>
    {#if snapshot}<span class="badge success"><span class="dot on"></span>Live</span>{/if}
  </header>

  {#if available === false}
    <div class="card">
      <EmptyState
        icon="live"
        title="Not available yet"
        text="This server does not report live matches yet. Until it does, the Server page lists the running game sessions and can end them."
      >
        <a class="btn" href={paths.server}>Open Server <Icon name="arrow" size={16} /></a>
      </EmptyState>
    </div>
  {:else if !snapshot}
    {#if error}
      <div class="notice error" role="alert"><Icon name="alert" /> {error}</div>
    {:else}
      <div class="card"><p class="muted">Loading…</p></div>
    {/if}
  {:else}
    {#if error}<div class="notice error" role="alert"><Icon name="alert" /> {error}</div>{/if}

    <div class="stats">
      <div class="stat"><span>Running matches</span><strong>{number(snapshot.matches.filter(item => !item.ended).length)}</strong></div>
      <div class="stat"><span>Players online</span><strong>{number(snapshot.lobby.online.length)}</strong></div>
      <div class="stat"><span>In a match</span><strong>{number(snapshot.lobby.online.filter(player => player.in_match).length)}</strong></div>
    </div>

    <section class="card">
      <div class="card-header"><h2>Matches</h2></div>
      {#if snapshot.matches.length}
        <div class="table-wrap">
          <table>
            <thead>
              <tr><th scope="col">Map</th><th scope="col">Mode</th><th scope="col">Phase</th><th scope="col">Time</th><th scope="col">Players</th><th></th></tr>
            </thead>
            <tbody>
              {#each snapshot.matches as item (item.id)}
                <tr>
                  <td><a href={matchPath(item.id)}><strong>{item.map}</strong></a></td>
                  <td>{item.mode}</td>
                  <td>
                    {#if item.ended}<span class="badge">Ended</span>
                    {:else if item.paused}<span class="badge warning">Paused</span>
                    {:else}<span class="badge success">{item.phase}</span>{/if}
                  </td>
                  <td class="numeric">{duration(item.clock)}</td>
                  <td>{number(item.players.filter(player => !player.ai).length)} + {number(item.players.filter(player => player.ai).length)} AI</td>
                  <td class="end"><a class="btn small" href={matchPath(item.id)}>Open</a></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>
      {:else}
        <EmptyState icon="live" title="No matches" text="A match shows here as soon as it starts." />
      {/if}
    </section>

    <form class="card" onsubmit={send}>
      <div class="card-header">
        <div>
          <h2>Message everyone</h2>
          <p>A system line in the chat of every player online.</p>
        </div>
      </div>
      <div class="broadcast">
        <input aria-label="Message" placeholder="The server restarts in 5 minutes" maxlength="200" bind:value={message} />
        <select aria-label="Who" bind:value={scope}>
          <option value="all">Everyone</option>
          <option value="lobby">Lobby only</option>
          <option value="matches">Matches only</option>
        </select>
        <button class="btn primary" type="submit" disabled={sending || !message.trim()}><Icon name="send" size={16} /> Send</button>
      </div>
    </form>
  {/if}
</div>

<style>
  .end {
    text-align: right;
  }

  .broadcast {
    display: grid;
    grid-template-columns: minmax(0, 1fr) 160px auto;
    gap: var(--space-3);
  }

  @media (max-width: 640px) {
    .broadcast {
      grid-template-columns: 1fr;
    }
  }
</style>
