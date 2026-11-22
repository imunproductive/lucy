<script lang="ts">
  import {ApiError} from '../lib/api.ts';
  import {app} from '../lib/app.svelte.ts';
  import {duration} from '../lib/format.ts';
  import * as live from '../lib/live.ts';
  import type {ChatChannel, ChatLine, FlowAction, FlowControl, LiveSnapshot} from '../lib/live.ts';
  import {paths} from '../lib/routes.ts';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';

  // One match: its players, its chat and its controls (stage 3). Until the
  // server has the routes, the page says so.
  let {id}: {id: string} = $props();

  let snapshot = $state<LiveSnapshot | null>(null);
  let available = $state<boolean | null>(null);
  let lines = $state<ChatLine[]>([]);
  let error = $state('');

  const match = $derived(snapshot?.matches.find(item => item.id === id) ?? null);

  $effect(() => {
    const matchId = id;
    let after: number | undefined;
    let stopped = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    lines = [];
    const readChat = async () => {
      try {
        const page = await live.getMatchChat(matchId, after);
        if (stopped) return;
        lines = live.appendChat(lines, page);
        after = page.next;
      } catch (failure) {
        if (!stopped && !(failure instanceof ApiError && failure.missing)) {
          error = failure instanceof Error ? failure.message : String(failure);
        }
      }
      if (!stopped) timer = setTimeout(readChat, 1000);
    };
    const poll = live.pollLive({
      update: next => {
        snapshot = next;
        if (!available) void readChat();
        available = true;
      },
      unavailable: () => (available = false),
      failed: failure => (error = failure instanceof Error ? failure.message : String(failure)),
    });
    return () => {
      stopped = true;
      clearTimeout(timer);
      poll.stop();
    };
  });

  const controlLabels: Partial<Record<FlowControl, string>> = {
    skip_phase: 'Skip phase',
    pause: 'Pause',
    unpause: 'Resume',
    draw: 'Draw',
    round_draw: 'Round draw',
    assemble: 'Assemble heroes',
    restart: 'Restart',
  };

  async function act(action: () => Promise<unknown>, done: string) {
    try {
      await action();
      app.toast(done);
    } catch (failure) {
      app.fail(failure);
    }
  }

  const flow = (action: FlowAction, done: string) => act(() => live.matchFlow(id, action), done);

  let chat = $state('');
  let channel = $state<ChatChannel>('match');
  let seconds = $state(60);

  async function sendChat(submit: SubmitEvent) {
    submit.preventDefault();
    const text = chat.trim();
    if (!text) return;
    await act(() => live.sendMatchChat(id, text, channel), 'Sent.');
    chat = '';
  }
</script>

<div class="page">
  {#if available === false}
    <header class="page-header">
      <div>
        <h1>Match <span class="mono muted">{id}</span></h1>
      </div>
    </header>
    <div class="card">
      <EmptyState icon="live" title="Not available yet" text="This server does not report live matches yet: the match page, its chat and its controls come with that.">
        <a class="btn" href={paths.matches}><Icon name="left" size={16} /> Live matches</a>
      </EmptyState>
    </div>
  {:else if !snapshot}
    {#if error}
      <div class="notice error" role="alert"><Icon name="alert" /> {error}</div>
    {:else}
      <div class="card"><p class="muted">Loading…</p></div>
    {/if}
  {:else if !match}
    <div class="card">
      <EmptyState icon="live" title="No such match" text="It ended, or the address is wrong.">
        <a class="btn" href={paths.matches}><Icon name="left" size={16} /> Live matches</a>
      </EmptyState>
    </div>
  {:else}
    <header class="page-header">
      <div>
        <h1>{match.map}</h1>
        <p>{match.mode} · {match.phase}{match.round ? ` · round ${match.round}` : ''}{match.overtime ? ' · overtime' : ''}</p>
      </div>
      <div class="actions">
        {#if match.ended}<span class="badge">Ended</span>{:else if match.paused}<span class="badge warning">Paused</span>{:else}<span class="badge success"
            ><span class="dot on"></span>Live</span
          >{/if}
        <span class="badge numeric"><Icon name="clock" size={13} /> {duration(match.clock)}</span>
      </div>
    </header>
    {#if error}<div class="notice error" role="alert"><Icon name="alert" /> {error}</div>{/if}

    <div class="columns">
      <section class="card chat-card">
        <div class="card-header"><h2>Chat</h2></div>
        <ol class="lines" aria-live="polite">
          {#each lines as line (line.seq)}
            <li class="line {line.kind}">
              <span class="who">{line.from?.name ?? 'System'}</span>
              {#if line.channel !== 'match'}<span class="badge">{line.channel === 'team1' ? 'Team 1' : 'Team 2'}</span>{/if}
              <span>{line.text}</span>
            </li>
          {:else}
            <li class="muted">No chat yet.</li>
          {/each}
        </ol>
        <form class="send" onsubmit={sendChat}>
          <select aria-label="Channel" bind:value={channel}>
            <option value="match">Match</option>
            <option value="team1">Team 1</option>
            <option value="team2">Team 2</option>
          </select>
          <input aria-label="Message" placeholder="Message as the system" maxlength="200" bind:value={chat} />
          <button class="btn primary" type="submit" disabled={!chat.trim()}><Icon name="send" size={16} /></button>
        </form>
      </section>

      <aside class="stack">
        <section class="card">
          <div class="card-header"><h2>Controls</h2></div>
          <div class="actions">
            {#each match.controls as control (control)}
              {#if controlLabels[control]}
                <button class="btn small" type="button" onclick={() => flow({action: control} as FlowAction, `${controlLabels[control]}: done.`)}>
                  {controlLabels[control]}
                </button>
              {/if}
            {/each}
          </div>
          {#if match.controls.includes('set_time')}
            <form
              class="time"
              onsubmit={event => {
                event.preventDefault();
                void flow({action: 'set_time', seconds}, 'Time set.');
              }}
            >
              <label class="field"><span>Match time (s)</span><input type="number" min="0" bind:value={seconds} /></label>
              <button class="btn small" type="submit">Set</button>
            </form>
          {/if}
          {#if match.controls.includes('declare_winner')}
            <div class="actions winners">
              <button class="btn small" type="button" onclick={() => flow({action: 'declare_winner', team: 1}, 'Team 1 wins.')}>Team 1 wins</button>
              <button class="btn small" type="button" onclick={() => flow({action: 'declare_winner', team: 2}, 'Team 2 wins.')}>Team 2 wins</button>
            </div>
          {/if}
          <div class="card-footer">
            <button class="btn danger" type="button" onclick={() => act(() => live.stopMatch(id, {result: 'none'}), 'The match is stopping.')}>
              <Icon name="stop" size={16} /> Stop match
            </button>
          </div>
        </section>

        <section class="card">
          <div class="card-header"><h2>Players</h2></div>
          <ul class="players">
            {#each match.players as player (player.account || player.name)}
              <li>
                <div>
                  <strong>{player.name}</strong>
                  <span class="muted">{player.hero} · team {player.team}{player.ai ? ' · AI' : ''}</span>
                </div>
                {#if !player.ai}
                  <button class="btn small danger" type="button" onclick={() => act(() => live.kick(id, player.account), `${player.name} was kicked.`)}>Kick</button>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      </aside>
    </div>
  {/if}
</div>

<style>
  .stack {
    display: grid;
    gap: var(--space-5);
  }

  .chat-card {
    display: flex;
    flex-direction: column;
    min-height: 420px;
  }

  .lines {
    flex: 1;
    display: grid;
    align-content: start;
    gap: var(--space-2);
    margin: 0 0 var(--space-4);
    padding: 0;
    list-style: none;
    max-height: 60vh;
    overflow-y: auto;
  }

  .line {
    display: flex;
    gap: var(--space-2);
    align-items: baseline;
  }

  .who {
    font-weight: 600;
  }

  .line.system .who,
  .line.notice .who {
    color: var(--accent);
  }

  .send {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr) auto;
    gap: var(--space-2);
  }

  .time {
    display: flex;
    align-items: flex-end;
    gap: var(--space-2);
    margin-top: var(--space-4);
  }

  .winners {
    margin-top: var(--space-4);
  }

  .players {
    display: grid;
    gap: var(--space-3);
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .players li {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-3);
  }

  .players div {
    display: grid;
  }

  .players span {
    font-size: 12.5px;
  }
</style>
