import {test} from 'node:test';
import assert from 'node:assert/strict';
import {subscribe} from './sse.ts';
import type {EventSourceLike, StreamStatus} from './sse.ts';

class FakeSource implements EventSourceLike {
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  closed = false;
  readonly listeners = new Map<string, ((event: MessageEvent) => void)[]>();
  readonly url: string;

  constructor(url: string) {
    this.url = url;
  }

  addEventListener(type: string, listener: (event: MessageEvent) => void): void {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), listener]);
  }

  close(): void {
    this.closed = true;
    this.readyState = 2;
  }

  emit(type: string, data: string): void {
    for (const listener of this.listeners.get(type) ?? []) listener({data} as MessageEvent);
  }

  open(): void {
    this.readyState = 1;
    this.onopen?.(new Event('open'));
  }

  fail(readyState: number): void {
    this.readyState = readyState;
    this.onerror?.(new Event('error'));
  }
}

test('events arrive parsed by name, and bad data is reported', () => {
  const sources: FakeSource[] = [];
  const got: unknown[] = [];
  const invalid: string[] = [];
  const statuses: StreamStatus[] = [];
  const close = subscribe<{live: {rev: number}}>('/api/events', {
    on: {live: data => got.push(data)},
    invalid: type => invalid.push(type),
    status: status => statuses.push(status),
    create: url => {
      const source = new FakeSource(url);
      sources.push(source);
      return source;
    },
  });
  assert.equal(sources[0].url, '/api/events');
  sources[0].open();
  sources[0].emit('live', '{"rev":4}');
  sources[0].emit('live', 'not json');
  close();
  assert.deepEqual(got, [{rev: 4}]);
  assert.deepEqual(invalid, ['live']);
  assert.deepEqual(statuses, ['connecting', 'open', 'closed']);
  assert.ok(sources[0].closed);
});

test('a stream the browser gave up on connects again after a growing wait', t => {
  t.mock.timers.enable({apis: ['setTimeout']});
  const sources: FakeSource[] = [];
  const statuses: StreamStatus[] = [];
  const close = subscribe('/api/events', {
    on: {},
    retry: 5,
    maxRetry: 10,
    status: status => statuses.push(status),
    create: url => {
      const source = new FakeSource(url);
      sources.push(source);
      return source;
    },
  });
  // A dropped connection the browser retries itself: nothing to do.
  sources[0].fail(0);
  assert.equal(sources.length, 1);
  sources[0].fail(2);
  t.mock.timers.tick(4);
  assert.equal(sources.length, 1, 'the first wait is 5 ms');
  t.mock.timers.tick(1);
  assert.equal(sources.length, 2);
  assert.ok(sources[0].closed);
  close();
  sources[1].fail(2);
  t.mock.timers.tick(100);
  assert.equal(sources.length, 2, 'a closed stream does not come back');
  assert.deepEqual(statuses, ['connecting', 'retrying', 'connecting', 'closed']);
});
