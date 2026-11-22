// The typed client of the dashboard's JSON API. Reads are GETs that skip the
// cache; actions are POSTs with a JSON body (the server refuses any other
// content type).
import type {
  BotAction,
  CollectionPage,
  CollectionQuery,
  Message,
  Ok,
  ProfileChanges,
  ProfileReply,
  Receipt,
  State,
} from './types.ts';

/** A refused request: the server's own message, and its HTTP status (0: no reply). */
export class ApiError extends Error {
  readonly status: number;

  constructor(message: string, status: number) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
  }

  /** The server has no such route (yet): a feature of a later backend. */
  get missing(): boolean {
    return this.status === 404 || this.status === 405;
  }
}

export async function request<T>(path: string, body?: object): Promise<T> {
  let response: Response;
  try {
    response = await fetch(
      path,
      body
        ? {method: 'POST', headers: {'Content-Type': 'application/json'}, body: JSON.stringify(body)}
        : {cache: 'no-store'},
    );
  } catch {
    throw new ApiError('Cannot reach the server. Is it running?', 0);
  }
  let result: unknown;
  try {
    result = await response.json();
  } catch {
    throw new ApiError('The server sent a reply that could not be read.', response.status);
  }
  const reply = (result ?? {}) as {status?: unknown; error?: unknown};
  if (!response.ok || reply.status === 'error' || reply.error) {
    const message = typeof reply.error === 'string' && reply.error ? reply.error : `Server error (${response.status}).`;
    throw new ApiError(message, response.status);
  }
  return result as T;
}

/** A query string from the set values; empty, false and undefined ones are left out. */
export function query(values: Record<string, string | number | boolean | undefined | null>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value === undefined || value === null || value === '' || value === false) continue;
    params.set(key, value === true ? '1' : String(value));
  }
  const text = params.toString();
  return text ? `?${text}` : '';
}

// Reads

/** The account's profile, every account, the server and the catalogs; no account: the selected one. */
export function getState(account?: string): Promise<State> {
  return request(`/api/state${query({account})}`);
}

export function getCollection(filter: CollectionQuery): Promise<CollectionPage> {
  return request(`/api/collection${query({...filter})}`);
}

// Profiles

export function updateProfile(account: string, changes: ProfileChanges): Promise<ProfileReply> {
  return request('/api/update_profile', {account, ...changes});
}

/** Set the lobby event or the hero challenge on every account (and new ones). */
export function applyToAll(change: {events: string[]} | {challenge: string}): Promise<Ok & {accounts: number}> {
  return request('/api/apply_to_all', change);
}

export function addBoxes(account: string, type: number, count: number): Promise<Ok & {added: number; total_boxes: number}> {
  return request('/api/add_boxes', {account, type, count});
}

export function openAllBoxes(account: string): Promise<ProfileReply & {opened: number; new_items: number}> {
  return request('/api/open_all_boxes', {account});
}

export function purchase(account: string, guid: string): Promise<ProfileReply & {receipt: Receipt}> {
  return request('/api/purchase', {account, guid});
}

/** Put a portrait frame on the card; null goes back to the level's frame. */
export function setFrame(account: string, guid: string | null): Promise<Ok & {frame: string | null}> {
  return request('/api/set_frame', {account, guid});
}

/** Unlock an item for free, or take back one unlocked here. */
export function grantItem(
  account: string,
  guid: string,
  revoke = false,
): Promise<ProfileReply & {granted: string[]; revoked: string[]; unlocked_count: number}> {
  return request('/api/grant_skin', revoke ? {account, guid, revoke: true} : {account, guid});
}

// Accounts and games

/** The account a game that logs in next plays. */
export function selectAccount(name: string): Promise<Ok> {
  return request('/api/select_account', {name});
}

/** The account the game logs in as each time the server starts. */
export function setDefaultAccount(name: string): Promise<Message> {
  return request('/api/default_account', {name});
}

/** Start a second game on this PC that plays the account. */
export function startGame(name: string): Promise<Ok & Message> {
  return request('/api/start_game', {name});
}

/** Drop the game played on this PC, so it logs in again as the selected account. */
export function reconnect(): Promise<Ok> {
  return request('/api/reconnect', {});
}

/** The group finder bot: list its group, join or invite the account's group, or leave. */
export function botGroup(account: string, action: BotAction): Promise<Ok & Message> {
  return request('/api/bot_group', {account, action});
}

// Matchmaking

/** Players to start a match in every mode without a number of its own; 0: full teams. */
export function setMatchmaking(testPlayers: number): Promise<Message> {
  return request('/api/matchmaking', {test_players: testPlayers});
}

export function setModeSettings(card: string, playersToStart: number, fillRunning: boolean): Promise<Message> {
  return request('/api/mode_settings', {card, players_to_start: playersToStart, fill_running: fillRunning});
}

/** The map every queue loads: a map GUID, or 'random' for each queue's own pick. */
export function setMap(map: string): Promise<Message> {
  return request('/api/set_map', {map});
}

/** The Workshop script file players go into from the menu; empty: none. */
export function setWorkshop(path: string): Promise<Message> {
  return request('/api/set_workshop', {path});
}

/** Send every running match's players back to the menu. */
export function endMatches(): Promise<Message> {
  return request('/api/end_matches', {});
}
