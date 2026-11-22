<script lang="ts">
  import * as api from '../lib/api.ts';
  import {app} from '../lib/app.svelte.ts';
  import {duration, number} from '../lib/format.ts';
  import type {BotAction, ModeRow, State} from '../lib/types.ts';
  import Icon from '../components/Icon.svelte';

  let {data}: {data: State} = $props();

  const server = $derived(data.server);

  // Each form keeps what was typed until it is saved; a refresh fills the
  // ones the user has not touched.
  let testPlayers = $state<number | null>(null);
  let map = $state<string | null>(null);
  let workshop = $state<string | null>(null);
  const testPlayersValue = $derived(testPlayers ?? server.test_players ?? 0);
  const mapValue = $derived(map ?? (server.forced_map || 'random'));
  const workshopValue = $derived(workshop ?? server.workshop);

  /** Per mode card: the typed players to start and join setting. */
  let modeEdits = $state<Record<string, {players: number; fill: boolean}>>({});

  let errors = $state<Record<string, string>>({});
  let busy = $state<string | null>(null);

  /** Run an action: its reply's message as a toast, its failure under its panel. */
  async function run(panel: string, action: () => Promise<{message?: string}>, after?: () => void) {
    busy = panel;
    errors[panel] = '';
    try {
      const result = await action();
      if (result.message) app.toast(result.message);
      after?.();
      await app.refresh();
    } catch (error) {
      errors[panel] = error instanceof Error ? error.message : String(error);
    } finally {
      busy = null;
    }
  }

  function modeEdit(mode: ModeRow) {
    return modeEdits[mode.card] ?? {players: mode.players_to_start, fill: mode.fill_running};
  }

  function editMode(mode: ModeRow, change: Partial<{players: number; fill: boolean}>) {
    modeEdits[mode.card] = {...modeEdit(mode), ...change};
  }

  function saveMode(mode: ModeRow) {
    const edit = modeEdit(mode);
    void run('modes', () => api.setModeSettings(mode.card, edit.players, edit.fill), () => delete modeEdits[mode.card]);
  }

  // Log the game in as another account: select it for the next login, show it here, reconnect the game.
  async function playAs(name: string) {
    try {
      await api.selectAccount(name);
      await api.reconnect();
      await app.show(name);
      app.toast(`The game logs in again as ${name}.`);
    } catch (error) {
      app.fail(error);
    }
  }

  async function secondGame(name: string) {
    try {
      app.toast((await api.startGame(name)).message);
    } catch (error) {
      app.fail(error);
    }
  }

  async function makeDefault(name: string) {
    try {
      app.toast((await api.setDefaultAccount(name)).message);
      await app.refresh();
    } catch (error) {
      app.fail(error);
    }
  }

  const botActions: [BotAction, string][] = [
    ['list', "List the bot's group"],
    ['join', 'Bot joins my group'],
    ['invite', 'Bot invites my group'],
    ['remove', 'Bot leaves'],
  ];
  let botBusy = $state<BotAction | null>(null);

  async function bot(action: BotAction) {
    botBusy = action;
    errors.bot = '';
    try {
      app.toast((await api.botGroup(app.account, action)).message);
    } catch (error) {
      errors.bot = error instanceof Error ? error.message : String(error);
    } finally {
      botBusy = null;
    }
  }
</script>

<div class="page">
  <header class="page-header">
    <div>
      <h1>Server</h1>
      <p>The local server, its matchmaking and the game processes.</p>
    </div>
    <span class="badge" class:success={app.connected} class:danger={app.connected === false}>
      <span class="dot" class:on={app.connected} class:off={app.connected === false}></span>
      {app.connected === false ? 'Data may be old' : 'Server is up'}
    </span>
  </header>

  <div class="stats">
    <div class="stat"><span>Lobby address</span><strong class="mono address">{server.host}:{server.port}</strong></div>
    <div class="stat"><span>Players connected</span><strong>{number(server.connected_clients)}</strong></div>
    <div class="stat"><span>Uptime</span><strong>{duration(server.uptime_seconds)}</strong></div>
    <div class="stat"><span>Running matches</span><strong>{number(server.game_instances.length)}</strong></div>
  </div>

  <div class="notice" class:success={server.matchmaking_supported}>
    <Icon name={server.matchmaking_supported ? 'check' : 'alert'} />
    {server.matchmaking_supported
      ? 'Matchmaking is on: a search that fills the teams starts a match on the game server.'
      : 'Lobby only: the game server is off.'}
  </div>

  <div class="grid-2">
    <section class="card">
      <div class="card-header">
        <div>
          <h2>Matchmaking</h2>
          <p>For every mode without a number of its own: 0 starts matches with full teams only; a number starts them as soon as that many players search.</p>
        </div>
      </div>
      <form
        class="inline-form"
        onsubmit={event => {
          event.preventDefault();
          void run('matchmaking', () => api.setMatchmaking(testPlayersValue), () => (testPlayers = null));
        }}
      >
        <label class="field">
          <span>Players to start</span>
          <input type="number" min="0" max="12" value={testPlayersValue} oninput={event => (testPlayers = Number(event.currentTarget.value))} />
        </label>
        <button class="btn" type="submit" disabled={busy === 'matchmaking'}>Save</button>
      </form>
      {#if errors.matchmaking}<p class="error-text" role="alert">{errors.matchmaking}</p>{/if}
      <div class="card-footer">
        <span class="hint grow">Sends the players of every running match back to the menu.</span>
        <button class="btn danger" type="button" disabled={busy === 'end'} onclick={() => run('end', () => api.endMatches())}>
          <Icon name="stop" size={16} /> End all matches
        </button>
      </div>
      {#if errors.end}<p class="error-text" role="alert">{errors.end}</p>{/if}
    </section>

    <div class="stack">
      <section class="card">
        <div class="card-header">
          <div>
            <h2>Map</h2>
            <p>The map every queue loads; Random keeps each queue's own pick. A queue whose modes the map is not played in keeps its pick; the Practice Range loads in every queue.</p>
          </div>
        </div>
        <form
          class="inline-form"
          onsubmit={event => {
            event.preventDefault();
            void run('map', () => api.setMap(mapValue), () => (map = null));
          }}
        >
          <label class="field">
            <span>Map</span>
            <select value={mapValue} onchange={event => (map = event.currentTarget.value)}>
              <option value="random">Random (the queue's pick)</option>
              {#each server.maps as item (item.guid)}<option value={item.guid}>{item.name}</option>{/each}
            </select>
          </label>
          <button class="btn" type="submit" disabled={busy === 'map'}>Save</button>
        </form>
        {#if errors.map}<p class="error-text" role="alert">{errors.map}</p>{/if}
      </section>

      <section class="card">
        <div class="card-header">
          <div>
            <h2>Workshop</h2>
            <p>A Workshop script (its text in a file): every player goes from the menu straight into the custom match its settings describe. Empty: off.</p>
          </div>
        </div>
        <form
          class="inline-form"
          onsubmit={event => {
            event.preventDefault();
            void run('workshop', () => api.setWorkshop(workshopValue), () => (workshop = null));
          }}
        >
          <label class="field">
            <span>Script file</span>
            <input value={workshopValue} autocomplete="off" placeholder="D:\Scripts\C32S2.txt" oninput={event => (workshop = event.currentTarget.value)} />
          </label>
          <button class="btn" type="submit" disabled={busy === 'workshop'}>Save</button>
        </form>
        {#if errors.workshop}<p class="error-text" role="alert">{errors.workshop}</p>{/if}
      </section>
    </div>
  </div>

  <section class="card">
    <div class="card-header">
      <div>
        <h2>Modes</h2>
        <p>
          A mode's own players to start (0 uses the number above), and whether players who search join its matches that are on. A match still waiting for its
          players takes them in every mode; competitive matches never once they are on.
        </p>
      </div>
    </div>
    {#if server.modes.length}
      <div class="table-wrap">
        <table>
          <thead>
            <tr><th scope="col">Mode</th><th scope="col">Teams</th><th scope="col">Players to start</th><th scope="col">Join games in progress</th><th></th></tr>
          </thead>
          <tbody>
            {#each server.modes as mode (mode.card)}
              {@const edit = modeEdit(mode)}
              <tr>
                <td><strong>{mode.name}</strong>{#if mode.competitive}<span class="badge accent comp">Competitive</span>{/if}</td>
                <td class="muted">{mode.teams}</td>
                <td>
                  <input
                    class="players"
                    type="number"
                    min="0"
                    max="12"
                    aria-label="{mode.name}: players to start"
                    value={edit.players}
                    oninput={event => editMode(mode, {players: Number(event.currentTarget.value)})}
                  />
                </td>
                <td>
                  <input
                    type="checkbox"
                    class="check"
                    aria-label="{mode.name}: join games in progress"
                    title={mode.competitive ? 'Competitive matches never take players once they are on.' : undefined}
                    disabled={mode.competitive}
                    checked={edit.fill}
                    onchange={event => editMode(mode, {fill: event.currentTarget.checked})}
                  />
                </td>
                <td class="end"><button class="btn small" type="button" disabled={busy === 'modes'} onclick={() => saveMode(mode)}>Save</button></td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else}
      <p class="muted">No modes: the game server is off.</p>
    {/if}
    {#if errors.modes}<p class="error-text" role="alert">{errors.modes}</p>{/if}
  </section>

  <section class="card">
    <div class="card-header">
      <h2>Accounts</h2>
      <span class="muted">{number(data.accounts.length)} accounts</span>
    </div>
    <div class="table-wrap">
      <table>
        <thead>
          <tr><th scope="col">Account</th><th scope="col">Connection</th><th scope="col">Selected</th><th scope="col">Game</th></tr>
        </thead>
        <tbody>
          {#each data.accounts as row (row.name)}
            <tr>
              <td>
                <strong>{row.name}</strong>
                {#if row.default}<span class="badge comp">Default</span>{/if}
              </td>
              <td>
                <span class="connection"><span class="dot" class:on={row.online}></span>{row.online ? 'Connected' : 'Not connected'}</span>
              </td>
              <td>{#if row.selected}<span class="badge accent">Selected</span>{:else}<span class="muted">—</span>{/if}</td>
              <td>
                <div class="actions">
                  <button class="btn small" type="button" disabled={row.online} onclick={() => playAs(row.name)}>
                    <Icon name="play" size={14} />{row.online ? 'Playing' : 'Play as'}
                  </button>
                  {#if server.second_games && !row.online}
                    <button class="btn small" type="button" onclick={() => secondGame(row.name)}>Second game</button>
                  {/if}
                  <button class="btn small ghost" type="button" disabled={row.default} onclick={() => makeDefault(row.name)}>
                    {row.default ? 'Default' : 'Make default'}
                  </button>
                </div>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
    </div>
  </section>

  <div class="grid-2">
    <section class="card">
      <div class="card-header">
        <div>
          <h2>Group finder bot</h2>
          <p>A second player for the group finder, for <strong>{app.account}</strong>.</p>
        </div>
      </div>
      <div class="actions">
        {#each botActions as [action, label] (action)}
          <button class="btn" type="button" disabled={botBusy !== null} onclick={() => bot(action)}>{label}</button>
        {/each}
      </div>
      {#if errors.bot}<p class="error-text bot-error" role="alert">{errors.bot}</p>{/if}
    </section>

    <section class="card">
      <div class="card-header">
        <h2>Game sessions</h2>
        <span class="muted">{number(server.game_instances.length)} matches</span>
      </div>
      {#each server.game_instances as instance, index (index)}
        <div class="session">
          <span class="session-icon"><Icon name="live" size={16} /></span>
          <div class="session-copy">
            <h3>{instance.name}</h3>
            <p class="muted">{instance.player ? `Players: ${instance.player}` : 'No players'} · UDP {instance.host}:{instance.port}</p>
          </div>
          <span class="badge">{instance.state}</span>
        </div>
      {:else}
        <p class="muted">No sessions.</p>
      {/each}
    </section>
  </div>
</div>

<style>
  .address {
    font-size: 18px !important;
  }

  .grid-2 {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: var(--space-5);
    align-items: start;
  }

  @media (max-width: 1080px) {
    .grid-2 {
      grid-template-columns: minmax(0, 1fr);
    }
  }

  .stack {
    display: grid;
    gap: var(--space-5);
  }

  .inline-form {
    display: flex;
    align-items: flex-end;
    gap: var(--space-3);
  }

  .inline-form .field {
    flex: 1;
  }

  .error-text {
    margin-top: var(--space-3);
  }

  .comp {
    margin-left: var(--space-2);
  }

  .players {
    width: 90px;
  }

  .check {
    width: 18px;
    height: 18px;
    accent-color: var(--accent);
  }

  .end {
    text-align: right;
  }

  .connection {
    display: inline-flex;
    align-items: center;
    gap: var(--space-2);
    white-space: nowrap;
  }

  .session {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) 0;
    border-bottom: 1px solid var(--border);
  }

  .session:last-child {
    border-bottom: 0;
  }

  .session-icon {
    display: grid;
    place-items: center;
    flex: none;
    width: 32px;
    height: 32px;
    border-radius: var(--radius);
    background: var(--accent-soft);
    color: var(--accent-soft-text);
  }

  .session-copy {
    flex: 1;
    min-width: 0;
  }

  .session-copy p {
    font-size: 12.5px;
    overflow-wrap: anywhere;
  }
</style>
