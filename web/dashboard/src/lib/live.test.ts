import {afterEach, test} from 'node:test';
import assert from 'node:assert/strict';
import * as live from './live.ts';
import type {ChatLine, LiveSnapshot} from './live.ts';

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

type Reply = {status: number; body: unknown};

function serve(answer: (url: string, init?: RequestInit) => Reply): {url: string; init?: RequestInit}[] {
  const calls: {url: string; init?: RequestInit}[] = [];
  globalThis.fetch = (async (url: string, init?: RequestInit) => {
    calls.push({url, init});
    const {status, body} = answer(url, init);
    return {ok: status < 300, status, json: async () => body} as Response;
  }) as typeof fetch;
  return calls;
}

const snapshot: LiveSnapshot = {rev: 3, lobby: {online: []}, matches: []};

test('the stage 3 routes and their bodies follow the API', async () => {
  const calls = serve(() => ({status: 200, body: {status: 'ok', lines: [], next: 0, rev: 1}}));
  await live.getLive();
  await live.getLive(7);
  await live.getMatchChat('ab12', 5);
  await live.getLobbyChat();
  await live.sendMatchChat('ab12', 'hello', 'team1');
  await live.broadcast({text: 'Restart in 5 min', scope: 'all'});
  await live.kick('ab12', '1234', 'AFK');
  await live.matchFlow('ab12', {action: 'set_time', seconds: 60});
  await live.stopMatch('ab12', {result: 'winner', team: 1});
  assert.deepEqual(
    calls.map(call => [call.url, call.init?.body ? JSON.parse(String(call.init.body)) : null]),
    [
      ['/api/live', null],
      ['/api/live?rev=7', null],
      ['/api/matches/ab12/chat?after=5', null],
      ['/api/lobby/chat', null],
      ['/api/matches/ab12/chat', {text: 'hello', channel: 'team1'}],
      ['/api/broadcast', {text: 'Restart in 5 min', scope: 'all'}],
      ['/api/matches/ab12/kick', {account: '1234', reason: 'AFK'}],
      ['/api/matches/ab12/flow', {action: 'set_time', seconds: 60}],
      ['/api/matches/ab12/stop', {result: 'winner', team: 1}],
    ],
  );
});

test('an unchanged reply keeps the snapshot', () => {
  assert.equal(live.mergeLive(snapshot, {rev: 3}), snapshot);
  assert.equal(live.mergeLive(null, {rev: 3}), null);
  const next = {...snapshot, rev: 4};
  assert.equal(live.mergeLive(snapshot, next), next);
});

test('chat pages append without repeating a line', () => {
  const line = (seq: number): ChatLine => ({seq, at: 0, kind: 'chat', channel: 'match', from: null, text: String(seq)});
  const lines = live.appendChat([line(1), line(2)], {lines: [line(2), line(3)], next: 3});
  assert.deepEqual(lines.map(item => item.seq), [1, 2, 3]);
});

/** Let the pending fetch replies (microtasks and I/O callbacks) run. */
function settle(): Promise<void> {
  const {promise, resolve} = Promise.withResolvers<void>();
  setImmediate(resolve);
  return promise;
}

test('polling sends the last rev and goes on after a failed poll', async t => {
  t.mock.timers.enable({apis: ['setTimeout']});
  let polls = 0;
  const calls = serve(() => {
    polls += 1;
    if (polls === 1) return {status: 200, body: snapshot};
    if (polls === 2) return {status: 503, body: {error: 'The lobby is not running.'}};
    return {status: 200, body: {rev: 3}};
  });
  const updates: LiveSnapshot[] = [];
  const failures: unknown[] = [];
  const poll = live.pollLive({update: value => updates.push(value), failed: error => failures.push(error), interval: 1000});
  for (let step = 0; step < 3; step++) {
    await settle();
    t.mock.timers.tick(1000);
  }
  await settle();
  poll.stop();
  assert.equal(failures.length, 1);
  assert.deepEqual(calls.map(call => call.url), ['/api/live', '/api/live?rev=3', '/api/live?rev=3', '/api/live?rev=3']);
  assert.deepEqual(updates, [snapshot]);
});
