<script lang="ts">
  import {app} from '../lib/app.svelte.ts';
  import {duration} from '../lib/format.ts';
  import * as live from '../lib/live.ts';
  import type {ChatChannel, ChatLine, FlowAction, FlowControl, LiveSnapshot, StopResult} from '../lib/live.ts';
  import {paths} from '../lib/routes.ts';
  import EmptyState from '../components/EmptyState.svelte';
  import Icon from '../components/Icon.svelte';

  // One match: its players, its chat and its controls (stage 3). The
  // controls are those its mode allows now (`controls`).
  let {id}: {id: string} = $props();

  let snapshot = $state<LiveSnapshot | null>(null);
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
        if (!stopped) error = failure instanceof Error ? failure.message : String(failure);
      }
      if (!stopped) timer = setTimeout(readChat, 1000);
    };
    void readChat();
    const poll = live.pollLive({
      update: next => {
        snapshot = next;
        error = '';
      },
      failed: failure => (error = failure instanceof Error ? failure.message : String(failure)),
    });
    return () => {
      stopped = true;
      clearTimeout(timer);
      poll.stop();
    };
  });

  /** The controls that take no value, with their buttons' labels. */
  const simple: [FlowControl, string][] = [
    ['skip_phase', 'Skip phase'],
    ['pause', 'Pause'],
    ['unpause', 'Resume'],
    ['assemble', 'Back to assembly'],
    ['round_draw', 'Round draw'],
  ];

  const teamName = (team: number) => `Team ${team + 1}`;

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
  let stopResult = $state<'none' | 'draw' | 'winner0' | 'winner1'>('none');
  let kickReason = $state('');

  async function sendChat(submit: SubmitEvent) {
    submit.preventDefault();
    const text = chat.trim();
    if (!text) return;
    await act(() => live.sendMatchChat(id, text, channel), 'Sent.');
    chat = '';
  }

  function stop() {
    const result: StopResult =
      stopResult === 'none' ? {result: 'none'} : stopResult === 'draw' ? {result: 'draw'} : {result: 'winner', team: stopResult === 'winner0' ? 0 : 1};
    void act(() => live.stopMatch(id, result), result.result === 'none' ? 'The players go back to the menu.' : 'The match ends with its result.');
  }

  function kick(account: string, name: string) {
    const reason = kickReason.trim();
    void act(() => live.kick(id, account, reason || undefined), `${name} was kicked.`);
  }
</script>

<div class="page">
  {#if !snapshot}
    {#if error}
      <div class="notice error" role="alert"><Icon name="alert" /> {error}</div>
    {:else}
      <div class="card"><p class="muted">Loading…</p></div>
    {/if}
  {:else if !match}
    <div class="card">
      <EmptyState icon="live" title="No such match" text="It ended, or the address is wrong. Its chat stays for half an hour after it ends.">
        <a class="btn" href={paths.matches}><Icon name="left" size={16} /> Live matches</a>
      </EmptyState>
    </div>
    {#if lines.length}
      <section class="card">
        <div class="card-header"><h2>Chat</h2></div>
        <ol class="lines">
          {#each lines as line (line.seq)}
            <li class="line {line.kind}"><span class="who">{line.from?.name ?? 'System'}</span> <span>{line.text}</span></li>
          {/each}
        </ol>
      </section>
    {/if}
  {:else}
    <header class="page-header">
      <div>
        <h1>{match.map}</h1>
        <p>
          {match.mode}{match.phase ? ` · ${match.phase.replace('_', ' ')}` : ''}{match.round ? ` · round ${match.round}` : ''}{match.overtime
            ? ' · overtime'
            : ''}{match.winner !== null ? ` · ${teamName(match.winner)} won` : ''}
        </p>
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
              {#if line.channel === 'team1' || line.channel === 'team2'}<span class="badge">{line.channel === 'team1' ? 'Team 1' : 'Team 2'}</span>{/if}
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
          <div class="card-header">
            <div>
              <h2>Controls</h2>
              <p>
                {match.driven === 'graph'
                  ? `The mode graph ${match.mode_graph} runs the match.`
                  : match.driven === 'flow'
                    ? 'The server’s match flow runs the match.'
                    : 'This mode has no match flow.'}
              </p>
            </div>
          </div>
          {#if match.controls.length}
            <div class="actions">
              {#each simple as [control, label] (control)}
                {#if match.controls.includes(control)}
                  <button class="btn small" type="button" onclick={() => flow({action: control} as FlowAction, `${label}: done.`)}>{label}</button>
                {/if}
              {/each}
              {#if match.controls.includes('completion')}
                <button
                  class="btn small"
                  type="button"
                  onclick={() =>
                    flow({action: 'completion', on: match.completion_off}, match.completion_off ? 'The match ends at clock 0 again.' : 'The match no longer ends at clock 0.')}
                >
                  {match.completion_off ? 'End at clock 0' : 'Keep on at clock 0'}
                </button>
              {/if}
            </div>
            {#if match.controls.includes('set_time')}
              <form
                class="time"
                onsubmit={event => {
                  event.preventDefault();
                  void flow({action: 'set_time', seconds}, 'Time set.');
                }}
              >
                <label class="field"><span>Time left in this phase (s)</span><input type="number" min="0" max="3599" bind:value={seconds} /></label>
                <button class="btn small" type="submit">Set</button>
              </form>
            {/if}
            {#if match.controls.includes('round_winner')}
              <div class="actions winners">
                {#each [0, 1] as team (team)}
                  <button class="btn small" type="button" onclick={() => flow({action: 'round_winner', team}, `${teamName(team)} wins the round.`)}
                    >{teamName(team)} wins round</button
                  >
                {/each}
              </div>
            {/if}
          {:else}
            <p class="muted">No flow control now.</p>
          {/if}
          <div class="card-footer stop">
            <select aria-label="Result" bind:value={stopResult}>
              <option value="none">No result: home now</option>
              {#if match.controls.includes('declare_winner')}
                <option value="winner0">Team 1 wins</option>
                <option value="winner1">Team 2 wins</option>
              {/if}
              {#if match.controls.includes('draw')}<option value="draw">Draw</option>{/if}
            </select>
            <button class="btn danger" type="button" onclick={stop}><Icon name="stop" size={16} /> Stop match</button>
          </div>
        </section>

        <section class="card">
          <div class="card-header"><h2>Players</h2></div>
          <ul class="players">
            {#each match.players as player (player.account || player.name)}
              <li>
                <div>
                  <strong>{player.name}</strong>
                  <span class="muted">{player.hero || 'No hero'} · {teamName(player.team)} · {player.state}{player.ai ? ' · AI' : ''}</span>
                </div>
                {#if !player.ai}
                  <button class="btn small danger" type="button" onclick={() => kick(player.account, player.name)}>Kick</button>
                {/if}
              </li>
            {/each}
          </ul>
          <label class="field reason"><span>Kick reason (optional)</span><input maxlength="120" bind:value={kickReason} /></label>
        </section>

        <section class="card">
          <div class="card-header">
            <div>
              <h2>Server ticks</h2>
              <p>{match.timing.ticks} ticks in the last {match.timing.span_s} s{match.timing.skipped ? `, ${match.timing.skipped} skipped` : ''}.</p>
            </div>
          </div>
          <dl class="timing">
            <dt>Work</dt>
            <dd class="numeric">{match.timing.work_mean_ms} ms mean · {match.timing.work_p99_ms} p99 · {match.timing.work_max_ms} max</dd>
            <dt>Start late</dt>
            <dd class="numeric">{match.timing.late_mean_ms} ms mean · {match.timing.late_p99_ms} p99</dd>
            {#each Object.entries(match.timing.phases_mean_ms).filter(([, ms]) => ms > 0) as [phase, ms] (phase)}
              <dt>{phase}</dt>
              <dd class="numeric">{ms} ms</dd>
            {/each}
          </dl>
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

  .stop {
    display: flex;
    gap: var(--space-2);
    flex-wrap: wrap;
  }

  .reason {
    margin-top: var(--space-4);
  }

  .timing {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: var(--space-1) var(--space-3);
    margin: 0;
    font-size: 13px;
  }

  .timing dt {
    color: var(--muted);
    text-transform: capitalize;
  }

  .timing dd {
    margin: 0;
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
