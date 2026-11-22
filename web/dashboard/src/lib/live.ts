// The live-match API of stage 3 (refs/reports/Dashboard.md §7; the server's
// `crates/server/src/server/dashboard/live.rs`).
import {query, request} from './api.ts';
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
  | 'completion';

export interface LiveMatch {
  /** 32 hex digits. */
  id: string;
  map: string;
  mode: string;
  /** The mode graph ("0F4A"), null for none. */
  mode_graph: string | null;
  driven: 'graph' | 'flow' | 'none';
  /** "assemble", "setup", "round", "round_end", "match_end", "waiting"; null before the flow starts and in the Range. */
  phase: string | null;
  /** Seconds on the match clock. */
  clock: number;
  paused: boolean;
  round: number;
  /** Teams are 0 and 1 ("Team 1", "Team 2"). */
  attackers: number | null;
  objective: number;
  overtime: boolean;
  winner: number | null;
  /** The mode's own end at clock 0 is off. */
  completion_off: boolean;
  /** A Workshop match's rule count, null for others. */
  workshop_rules: number | null;
  /** Unix time in seconds. */
  started_at: number;
  ended: boolean;
  players: MatchPlayer[];
  chat_seq: number;
  controls: FlowControl[];
  /** The room's tick timing: its last 10 s window (all ticks before the first closes). */
  timing: TickTiming;
}

export interface TickTiming {
  ticks: number;
  span_s: number;
  skipped: number;
  work_mean_ms: number;
  work_p99_ms: number;
  work_max_ms: number;
  late_mean_ms: number;
  late_p99_ms: number;
  /** Mean milliseconds per tick in each part of the work (simulate, script, replicate…). */
  phases_mean_ms: Record<string, number>;
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
  kind: 'chat' | 'system';
  channel: ChatChannel | 'general';
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
  /** Team 0 or 1, or the team of the player with this account. */
  | {action: 'declare_winner'; team: number}
  | {action: 'declare_winner'; account: string}
  | {action: 'draw'}
  | {action: 'round_winner'; team: number}
  | {action: 'round_draw'}
  | {action: 'assemble'}
  | {action: 'completion'; on: boolean};

export type StopResult = {result: 'winner'; team: number} | {result: 'draw'} | {result: 'none'};

export interface Broadcast {
  text: string;
  scope: 'all' | 'lobby' | 'matches';
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

export interface LivePoll {
  stop(): void;
}

/**
 * Poll `/api/live` every `interval` ms with the last rev, and call `update`
 * with each new snapshot. A failed poll calls `failed` and the next one
 * tries again.
 */
export function pollLive(handlers: {
  update(snapshot: LiveSnapshot): void;
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
