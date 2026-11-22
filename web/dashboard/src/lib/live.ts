// The live-match API of stage 3 (refs/reports/Dashboard.md §7). The server
// does not have these routes yet: each call then fails with an ApiError whose
// `missing` is true, and the pages say the feature is not available yet.
import {ApiError, query, request} from './api.ts';
import type {Ok} from './types.ts';

export interface OnlinePlayer {
  account: string;
  name: string;
  in_match: boolean;
}

export interface MatchPlayer {
  account: string;
  name: string;
  hero: string;
  team: number;
  state: string;
  ai: boolean;
}

/** What a match's flow can be told to do; `controls` lists the ones its mode allows. */
export type FlowControl =
  | 'set_time'
  | 'skip_phase'
  | 'pause'
  | 'unpause'
  | 'declare_winner'
  | 'draw'
  | 'round_winner'
  | 'round_draw'
  | 'assemble'
  | 'restart'
  | 'completion';

export interface LiveMatch {
  /** 32 hex digits. */
  id: string;
  map: string;
  mode: string;
  /** The mode graph ("0F4A"), null for none. */
  mode_graph: string | null;
  driven: 'graph' | 'flow' | 'none';
  phase: string;
  /** Seconds on the match clock. */
  clock: number;
  paused: boolean;
  round: number;
  attackers: number | null;
  objective: unknown;
  overtime: boolean;
  winner: number | null;
  workshop_rules: unknown;
  /** Unix time in seconds. */
  started_at: number;
  ended: boolean;
  players: MatchPlayer[];
  chat_seq: number;
  controls: FlowControl[];
}

export interface LiveSnapshot {
  rev: number;
  lobby: {online: OnlinePlayer[]};
  matches: LiveMatch[];
}

/** `GET /api/live?rev=N`: the whole snapshot, or only `rev` when nothing changed since N. */
export type LiveReply = LiveSnapshot | {rev: number};

export type ChatChannel = 'match' | 'team1' | 'team2';

export interface ChatLine {
  seq: number;
  /** Unix time in seconds. */
  at: number;
  kind: 'chat' | 'system' | 'notice' | 'command';
  channel: ChatChannel;
  from: {account: string; name: string} | null;
  text: string;
}

export interface ChatPage {
  lines: ChatLine[];
  /** The `after` of the next request. */
  next: number;
}

export type FlowAction =
  | {action: 'set_time'; seconds: number}
  | {action: 'skip_phase'}
  | {action: 'pause'}
  | {action: 'unpause'}
  | {action: 'declare_winner'; team: number}
  | {action: 'declare_winner'; account: string}
  | {action: 'draw'}
  | {action: 'round_winner'; team: number}
  | {action: 'round_draw'}
  | {action: 'assemble'}
  | {action: 'restart'}
  | {action: 'completion'; on: boolean};

export type StopResult = {result: 'winner'; team: number} | {result: 'draw'} | {result: 'none'};

export interface Broadcast {
  text: string;
  scope: 'all' | 'lobby' | 'matches';
  hud?: 'none' | 'small' | 'big';
}

const match = (id: string) => `/api/matches/${encodeURIComponent(id)}`;

export function getLive(rev?: number): Promise<LiveReply> {
  return request(`/api/live${query({rev})}`);
}

export function getMatchChat(id: string, after?: number): Promise<ChatPage> {
  return request(`${match(id)}/chat${query({after})}`);
}

export function getLobbyChat(after?: number): Promise<ChatPage> {
  return request(`/api/lobby/chat${query({after})}`);
}

/** A line from the system to the match (or one team's) chat. */
export function sendMatchChat(id: string, text: string, channel?: ChatChannel): Promise<Ok> {
  return request(`${match(id)}/chat`, channel ? {text, channel} : {text});
}

export function broadcast(message: Broadcast): Promise<Ok> {
  return request('/api/broadcast', message);
}

export function kick(id: string, account: string, reason?: string): Promise<Ok> {
  return request(`${match(id)}/kick`, reason ? {account, reason} : {account});
}

/** 409 when the match's `controls` lack the action. */
export function matchFlow(id: string, action: FlowAction): Promise<Ok> {
  return request(`${match(id)}/flow`, action);
}

export function stopMatch(id: string, result: StopResult): Promise<Ok> {
  return request(`${match(id)}/stop`, result);
}

/** A reply that only has `rev` means nothing changed: keep the snapshot. */
export function mergeLive(previous: LiveSnapshot | null, reply: LiveReply): LiveSnapshot | null {
  return 'matches' in reply ? reply : previous;
}

/** Chat pages appended in order, without a line twice. */
export function appendChat(lines: ChatLine[], page: ChatPage): ChatLine[] {
  const last = lines.at(-1)?.seq ?? -Infinity;
  return [...lines, ...page.lines.filter(line => line.seq > last)];
}

export type Availability<T> = {available: true; value: T} | {available: false};

/** The call's value, or `available: false` when the server lacks the route. Other errors stay errors. */
export async function ifAvailable<T>(call: Promise<T>): Promise<Availability<T>> {
  try {
    return {available: true, value: await call};
  } catch (error) {
    if (error instanceof ApiError && error.missing) return {available: false};
    throw error;
  }
}

export interface LivePoll {
  stop(): void;
}

/**
 * Poll `/api/live` every `interval` ms with the last rev, and call `update`
 * with each new snapshot. Stops by itself once the server lacks the route
 * (`unavailable`); a failed poll calls `failed` and the next one tries again.
 */
export function pollLive(handlers: {
  update(snapshot: LiveSnapshot): void;
  unavailable?(): void;
  failed?(error: unknown): void;
  interval?: number;
}): LivePoll {
  let snapshot: LiveSnapshot | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let stopped = false;
  const tick = async () => {
    try {
      const reply = await getLive(snapshot?.rev);
      if (stopped) return;
      const next = mergeLive(snapshot, reply);
      if (next && next !== snapshot) handlers.update(next);
      snapshot = next;
    } catch (error) {
      if (stopped) return;
      if (error instanceof ApiError && error.missing) {
        stopped = true;
        handlers.unavailable?.();
        return;
      }
      handlers.failed?.(error);
    }
    if (!stopped) timer = setTimeout(tick, handlers.interval ?? 1000);
  };
  void tick();
  return {
    stop() {
      stopped = true;
      clearTimeout(timer);
    },
  };
}
