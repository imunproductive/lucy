<script lang="ts">
  import {untrack} from 'svelte';
  import * as api from '../lib/api.ts';
  import {app} from '../lib/app.svelte.ts';
  import {changes, draftOf, rebase} from '../lib/draft.ts';
  import {FRAME_TIERS, currencies, frameForLevel, frameName, levelParts, number, totalLevel} from '../lib/format.ts';
  import {paths} from '../lib/routes.ts';
  import type {Currency, Profile, Queue, State} from '../lib/types.ts';
  import Coin from '../components/Coin.svelte';
  import FrameArt from '../components/FrameArt.svelte';
  import Icon from '../components/Icon.svelte';
  import Toggle from '../components/Toggle.svelte';

  let {data}: {data: State} = $props();

  const queues: [Queue, string][] = [
    ['tank', 'Tank'],
    ['damage', 'Damage'],
    ['support', 'Support'],
    ['open', 'Open queue'],
    ['ctf', 'Competitive CTF'],
    ['lucio', 'Lucio Cup'],
  ];
  const coins: Currency[] = ['credits', 'comp_points', 'league_tokens'];
  const regions = [
    ['US', 'United States (USD)'],
    ['EU', 'Europe (EUR)'],
    ['GB', 'United Kingdom (GBP)'],
    ['RU', 'Russia (RUB)'],
  ];
  const gameRegions = [
    ['americas', 'Americas'],
    ['europe', 'Europe'],
    ['asia', 'Asia'],
  ];
  const seasons = Array.from({length: 32}, (_, index) => 32 - index);

  const fields = [
    'player_name',
    'region',
    'game_region',
    'level',
    'endorsement_level',
    'credits',
    'comp_points',
    'league_tokens',
    'season',
    ...queues.flatMap(([queue]) => [`rating_${queue}`, `matches_${queue}`, `wins_${queue}`] as const),
    'passes_quick_play',
    'passes_competitive',
    'lobby_hero',
    'server_date',
    'unlock_all',
    'bot_chat',
    'sms_protect',
  ] as const satisfies readonly (keyof Profile)[];

  const profile = $derived(data.profile);
  // The draft starts from the first profile; the effect below moves it to the next ones.
  let base: Profile = untrack(() => data.profile);
  let draft = $state(draftOf(base, fields));
  let saving = $state(false);
  let error = $state('');

  // A refresh brings new values: the untouched fields take them.
  $effect.pre(() => {
    const next = profile;
    if (next === base) return;
    rebase(draft, base, next, fields);
    base = next;
  });

  const dirty = $derived(changes(draft, profile, fields));
  const dirtyCount = $derived(Object.keys(dirty).length);
  const level = $derived(Math.max(1, Number(draft.level) || 1));
  const parts = $derived(levelParts(level));
  // A changed level brings back the level's frame when saved, so the preview shows that one.
  const chosen = $derived('level' in dirty ? null : (data.catalogs.frames.find(frame => frame.guid === profile.frame) ?? null));
  const previewFrame = $derived(chosen ?? frameForLevel(data.catalogs.frames, level));
  const savedFrame = $derived(
    data.catalogs.frames.find(frame => frame.guid === profile.frame) ?? frameForLevel(data.catalogs.frames, profile.level),
  );
  const online = $derived(data.accounts.find(row => row.name === app.account)?.online ?? false);
  const event = $derived(data.catalogs.events.find(item => item.id === profile.events[0]));
  const challenge = $derived(data.catalogs.challenges.find(item => item.id === profile.challenge));

  function setLevel(change: Partial<typeof parts>) {
    draft.level = totalLevel({...parts, ...change});
  }

  function heroName(value: string): string {
    if (value === 'random') return 'Random hero';
    if (value === 'none') return 'No hero';
    return value || '—';
  }

  async function save(submit: SubmitEvent) {
    submit.preventDefault();
    const form = submit.currentTarget as HTMLFormElement;
    if (saving || !dirtyCount || !form.reportValidity()) return;
    const account = app.account;
    saving = true;
    error = '';
    const release = app.hold();
    try {
      await api.updateProfile(account, dirty);
      await app.refresh();
      app.toast('Profile saved.');
    } catch (failure) {
      error = failure instanceof Error ? failure.message : String(failure);
    } finally {
      saving = false;
      release();
    }
  }
</script>

<div class="page">
  <section class="card hero">
    <FrameArt guid={savedFrame?.guid ?? null} level={levelParts(profile.level).shown} size={88} />
    <div class="who">
      <h1>{profile.player_name || app.account}</h1>
      <div class="meta">
        <span class="badge" class:success={online}><span class="dot" class:on={online}></span>{online ? 'In the game' : 'Not connected'}</span>
        <span class="muted">Level {number(profile.level)}</span>
        {#if data.battle_tag}<span class="muted mono">{data.battle_tag}</span>{/if}
      </div>
      <p class="muted">{online ? 'The game is connected as this account.' : 'Start the game to connect it.'}</p>
    </div>
    <div class="wallet" aria-label="Currencies">
      {#each coins as coin (coin)}
        <div class="purse">
          <Coin currency={coin} size={30} />
          <div>
            <span class="muted">{currencies[coin].label}</span>
            <strong class="numeric">{number(profile[coin])}</strong>
          </div>
        </div>
      {/each}
    </div>
  </section>

  <div class="columns">
    <form class="card" onsubmit={save} aria-busy={saving}>
      <div class="card-header">
        <div>
          <h2>Profile</h2>
          <p>What the game shows for <strong>{app.account}</strong>. Only the changed fields are saved.</p>
        </div>
        {#if dirtyCount}<span class="badge warning">{dirtyCount} unsaved {dirtyCount === 1 ? 'change' : 'changes'}</span>{/if}
      </div>

      <fieldset disabled={saving}>
        <h3 class="section-label">Identity</h3>
        <div class="form-grid">
          <label class="field span-2">
            <span>Nickname</span>
            <input bind:value={draft.player_name} maxlength="32" autocomplete="off" required />
            <span class="hint">BattleTag for friends: {data.battle_tag || '—'}</span>
          </label>
          <label class="field">
            <span>Region</span>
            <select bind:value={draft.region}>
              {#each regions as [code, label] (code)}<option value={code}>{label}</option>{/each}
            </select>
            <span class="hint">Shop currency and country.</span>
          </label>
          <label class="field">
            <span>Game region</span>
            <select bind:value={draft.game_region}>
              {#each gameRegions as [code, label] (code)}<option value={code}>{label}</option>{/each}
            </select>
            <span class="hint">Game servers and the Top 500 board.</span>
          </label>
          <label class="field">
            <span>Lobby hero</span>
            <select bind:value={draft.lobby_hero}>
              <option value="random">Random hero</option>
              {#each data.catalogs.heroes as hero (hero.guid)}<option value={hero.name}>{hero.name}</option>{/each}
              <option value="none">No hero</option>
              <optgroup label="PvE characters (bind pose)">
                {#each data.catalogs.npcs as npc (npc)}<option value={npc}>{npc}</option>{/each}
              </optgroup>
            </select>
          </label>
          <label class="field">
            <span>Server date</span>
            <input bind:value={draft.server_date} placeholder="now or YYYY-MM-DD" autocomplete="off" />
            <span class="hint">Type now, or a date like 2022-10-03.</span>
          </label>
        </div>

        <h3 class="section-label">Level and frame</h3>
        <div class="level-row">
          <FrameArt guid={previewFrame?.guid ?? null} level={parts.shown} size={104} />
          <div class="level-controls">
            <div class="form-grid">
              <label class="field">
                <span>Level</span>
                <input type="number" min="1" max="100" value={parts.shown} oninput={event => setLevel({shown: Number(event.currentTarget.value)})} />
              </label>
              <label class="field">
                <span>Tier</span>
                <select value={parts.tier} onchange={event => setLevel({tier: Number(event.currentTarget.value)})}>
                  {#each FRAME_TIERS as tier, index (tier)}<option value={index}>{tier}</option>{/each}
                </select>
              </label>
              <label class="field">
                <span>Stars</span>
                <select value={parts.stars} onchange={event => setLevel({stars: Number(event.currentTarget.value)})}>
                  <option value={0}>No stars</option>
                  {#each [1, 2, 3, 4, 5] as stars (stars)}<option value={stars}>{'★'.repeat(stars)}</option>{/each}
                </select>
              </label>
              <label class="field">
                <span>Endorsement level</span>
                <select bind:value={draft.endorsement_level}>
                  {#each [1, 2, 3, 4, 5] as endorsement (endorsement)}<option value={endorsement}>{endorsement}</option>{/each}
                </select>
              </label>
            </div>
            <p class="hint">
              Total level {number(level)}. Frame: {previewFrame ? frameName(Math.max(1, previewFrame.level)) : '—'}{chosen
                ? ', chosen in Collection → Portrait frames'
                : ''}.
            </p>
          </div>
        </div>

        <h3 class="section-label">Currencies</h3>
        <div class="form-grid">
          {#each coins as coin (coin)}
            <label class="field">
              <span>{currencies[coin].label}</span>
              <input type="number" min="0" required bind:value={draft[coin]} />
            </label>
          {/each}
        </div>

        <h3 class="section-label">Competitive</h3>
        <div class="form-grid">
          <label class="field">
            <span>Season</span>
            <select bind:value={draft.season}>
              {#each seasons as season (season)}<option value={season}>{season}</option>{/each}
            </select>
            <span class="hint">Seasons from 18 have roles.</span>
          </label>
        </div>
        <div class="table-wrap queues">
          <table>
            <thead>
              <tr><th scope="col">Queue</th><th scope="col">Rating</th><th scope="col">Matches</th><th scope="col">Wins</th></tr>
            </thead>
            <tbody>
              {#each queues as [queue, label] (queue)}
                <tr>
                  <th scope="row">{label}</th>
                  <td><input type="number" min="1" max="5000" required aria-label="{label} rating" bind:value={draft[`rating_${queue}`]} /></td>
                  <td><input type="number" min="0" max="9999" required aria-label="{label} matches" bind:value={draft[`matches_${queue}`]} /></td>
                  <td><input type="number" min="0" max="9999" required aria-label="{label} wins" bind:value={draft[`wins_${queue}`]} /></td>
                </tr>
              {/each}
            </tbody>
          </table>
        </div>

        <h3 class="section-label">Priority passes</h3>
        <div class="form-grid">
          <label class="field">
            <span>Quick Play</span>
            <input type="number" min="0" max="40" required bind:value={draft.passes_quick_play} />
          </label>
          <label class="field">
            <span>Competitive</span>
            <input type="number" min="0" max="40" required bind:value={draft.passes_competitive} />
          </label>
        </div>

        <h3 class="section-label">Options</h3>
        <div class="toggles">
          <Toggle bind:checked={draft.unlock_all} label="Unlock the whole collection" description="Every item counts as owned." />
          <Toggle bind:checked={draft.bot_chat} label="Bot talks" description="The bot answers in chat, whispers and your group." />
          <Toggle bind:checked={draft.sms_protect} label="SMS Protect" description="Top 500 lists only accounts with it, a 3000+ rating and 25 wins." />
        </div>
      </fieldset>

      <div class="card-footer">
        {#if error}<span class="error-text grow" role="alert">{error}</span>{/if}
        {#if dirtyCount}
          <button class="btn ghost" type="button" disabled={saving} onclick={() => (draft = draftOf(profile, fields))}>Discard</button>
        {/if}
        <button class="btn primary" type="submit" disabled={saving || !dirtyCount}>{saving ? 'Saving…' : 'Save profile'}</button>
      </div>
    </form>

    <aside class="aside">
      <section class="card">
        <div class="card-header"><h2>In the lobby</h2></div>
        <dl class="details">
          <div><dt>Event</dt><dd>{event?.label ?? (profile.events[0] || 'No event')}</dd></div>
          <div><dt>Hero</dt><dd>{heroName(profile.lobby_hero)}</dd></div>
          <div><dt>Challenge</dt><dd>{challenge?.title ?? (profile.challenge || 'No challenge')}</dd></div>
          <div><dt>Server date</dt><dd>{!profile.server_date || profile.server_date === 'now' ? 'Current date' : profile.server_date}</dd></div>
        </dl>
        <a class="btn wide" href={paths.events}>Change event <Icon name="arrow" size={16} /></a>
      </section>
      <section class="card">
        <div class="card-header"><h2>Inventory</h2></div>
        <dl class="details">
          <div><dt>Loot boxes</dt><dd class="numeric">{number(profile.loot_boxes_count)}</dd></div>
          <div><dt>Items in collection</dt><dd class="numeric">{number(profile.unlocked_count)}</dd></div>
          <div><dt>Boxes opened</dt><dd class="numeric">{number(profile.stats?.boxes_opened)}</dd></div>
        </dl>
        <div class="actions">
          <a class="btn" href={paths.boxes}>Give boxes</a>
          <a class="btn ghost" href={paths.collection}>Collection</a>
        </div>
      </section>
    </aside>
  </div>
</div>

<style>
  .hero {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-5);
    background:
      radial-gradient(120% 140% at 0% 0%, color-mix(in srgb, var(--accent) 14%, transparent), transparent 55%),
      var(--surface);
  }

  .who {
    display: grid;
    gap: var(--space-2);
    flex: 1 1 260px;
    min-width: 0;
  }

  .who h1 {
    font-size: 26px;
    overflow-wrap: anywhere;
  }

  .meta {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-2) var(--space-3);
  }

  .wallet {
    display: grid;
    grid-template-columns: repeat(3, auto);
    gap: var(--space-3);
  }

  .purse {
    display: flex;
    align-items: center;
    gap: var(--space-3);
    padding: var(--space-3) var(--space-4);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--surface);
  }

  .purse div {
    display: grid;
  }

  .purse span {
    font-size: 12px;
  }

  .purse strong {
    font-size: 18px;
    font-weight: 650;
  }

  @media (max-width: 760px) {
    .wallet {
      grid-template-columns: 1fr;
      width: 100%;
    }
  }

  fieldset {
    border: 0;
    margin: 0;
    padding: 0;
    min-width: 0;
  }

  .level-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-5);
  }

  .level-controls {
    display: grid;
    gap: var(--space-2);
    flex: 1 1 320px;
  }

  .queues {
    margin-top: var(--space-4);
  }

  .queues th[scope='row'] {
    font-weight: 560;
    color: var(--text);
    font-size: 13.5px;
    padding-right: var(--space-4);
    border-bottom: 1px solid var(--border);
    vertical-align: middle;
  }

  .queues td {
    padding: var(--space-2);
  }

  .queues tr:last-child th {
    border-bottom: 0;
  }

  .queues input {
    min-width: 80px;
  }

  .toggles {
    display: grid;
    gap: var(--space-4);
  }

  .aside {
    display: grid;
    gap: var(--space-5);
  }

  .details {
    display: grid;
    gap: var(--space-3);
    margin: 0 0 var(--space-5);
  }

  .details div {
    display: flex;
    justify-content: space-between;
    gap: var(--space-4);
  }

  dt {
    color: var(--muted);
  }

  dd {
    margin: 0;
    font-weight: 560;
    text-align: right;
  }
</style>
