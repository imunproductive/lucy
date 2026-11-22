import {afterEach, test} from 'node:test';
import assert from 'node:assert/strict';
import * as api from './api.ts';
import {ApiError} from './api.ts';

interface Call {
  url: string;
  init: RequestInit | undefined;
}

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

/** Answer every request with `reply` (status 200 unless given), recording the calls. */
function serve(reply: unknown, status = 200): Call[] {
  const calls: Call[] = [];
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    calls.push({url, init});
    return {ok: status >= 200 && status < 300, status, json: async () => reply} as Response;
  }) as typeof fetch;
  return calls;
}

function body(call: Call): unknown {
  return JSON.parse(String(call.init?.body));
}

test('account selection is committed to the server as a JSON POST', async () => {
  const calls = serve({status: 'ok'});
  await api.selectAccount('Beta');
  assert.equal(calls.length, 1);
  assert.equal(calls[0].url, '/api/select_account');
  assert.equal(calls[0].init?.method, 'POST');
  assert.deepEqual(calls[0].init?.headers, {'Content-Type': 'application/json'});
  assert.deepEqual(body(calls[0]), {name: 'Beta'});
});

test('a refused account switch remains an error with the server message', async () => {
  serve({error: 'Profile not found'}, 404);
  await assert.rejects(api.selectAccount('Missing'), (error: unknown) => {
    assert.ok(error instanceof ApiError);
    assert.equal(error.message, 'Profile not found');
    assert.equal(error.status, 404);
    return true;
  });
});

test('reads skip the cache and name the account in the query', async () => {
  const calls = serve({profile: {}});
  await api.getState('Café Bar');
  assert.equal(calls[0].url, '/api/state?account=Caf%C3%A9+Bar');
  assert.equal(calls[0].init?.cache, 'no-store');
  assert.equal(calls[0].init?.method, undefined);
  await api.getState();
  assert.equal(calls[1].url, '/api/state');
});

test('the collection query leaves out blank filters and sends OWL as 1', async () => {
  const calls = serve({items: [], page: 1, pages: 1, total: 0});
  await api.getCollection({account: 'Alpha', kind: 'skins', q: '', hero: 'Ana', currency: '', owl: true, page: 2});
  assert.equal(calls[0].url, '/api/collection?account=Alpha&kind=skins&hero=Ana&owl=1&page=2');
  await api.getCollection({account: 'Alpha', owl: false});
  assert.equal(calls[1].url, '/api/collection?account=Alpha');
});

test('each action posts its fields to its route', async () => {
  const calls = serve({status: 'ok', message: 'done'});
  await api.updateProfile('Alpha', {credits: 5, events: ['halloween']});
  await api.applyToAll({challenge: 'Ana'});
  await api.addBoxes('Alpha', 3, 10);
  await api.openAllBoxes('Alpha');
  await api.purchase('Alpha', '0x10');
  await api.setFrame('Alpha', null);
  await api.grantItem('Alpha', '0x11');
  await api.grantItem('Alpha', '0x11', true);
  await api.setDefaultAccount('Alpha');
  await api.startGame('Beta');
  await api.reconnect();
  await api.botGroup('Alpha', 'join');
  await api.setMatchmaking(2);
  await api.setModeSettings('0x1A', 4, true);
  await api.setMap('random');
  await api.setWorkshop('D:\\Scripts\\C32S2.txt');
  await api.endMatches();
  assert.deepEqual(
    calls.map(call => [call.url, body(call)]),
    [
      ['/api/update_profile', {account: 'Alpha', credits: 5, events: ['halloween']}],
      ['/api/apply_to_all', {challenge: 'Ana'}],
      ['/api/add_boxes', {account: 'Alpha', type: 3, count: 10}],
      ['/api/open_all_boxes', {account: 'Alpha'}],
      ['/api/purchase', {account: 'Alpha', guid: '0x10'}],
      ['/api/set_frame', {account: 'Alpha', guid: null}],
      ['/api/grant_skin', {account: 'Alpha', guid: '0x11'}],
      ['/api/grant_skin', {account: 'Alpha', guid: '0x11', revoke: true}],
      ['/api/default_account', {name: 'Alpha'}],
      ['/api/start_game', {name: 'Beta'}],
      ['/api/reconnect', {}],
      ['/api/bot_group', {account: 'Alpha', action: 'join'}],
      ['/api/matchmaking', {test_players: 2}],
      ['/api/mode_settings', {card: '0x1A', players_to_start: 4, fill_running: true}],
      ['/api/set_map', {map: 'random'}],
      ['/api/set_workshop', {path: 'D:\\Scripts\\C32S2.txt'}],
      ['/api/end_matches', {}],
    ],
  );
  assert.ok(calls.every(call => call.init?.method === 'POST'));
});

test('an error status inside a 200 reply is still an error', async () => {
  serve({status: 'error', error: 'No boxes to open'});
  await assert.rejects(api.openAllBoxes('Alpha'), /No boxes to open/);
});

test('a failed reply without a message names its status', async () => {
  serve({}, 500);
  await assert.rejects(api.reconnect(), (error: unknown) => error instanceof ApiError && error.message === 'Server error (500).' && error.status === 500);
});

test('an unreachable server and an unreadable reply have their own messages', async () => {
  globalThis.fetch = (async () => {
    throw new TypeError('Failed to fetch');
  }) as typeof fetch;
  await assert.rejects(api.getState(), (error: unknown) => error instanceof ApiError && error.status === 0 && /Is it running/.test(error.message));
  globalThis.fetch = (async () =>
    ({
      ok: true,
      status: 200,
      json: async () => {
        throw new SyntaxError('bad');
      },
    }) as unknown as Response) as typeof fetch;
  await assert.rejects(api.getState(), /could not be read/);
});
