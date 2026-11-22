// What the dashboard API sends (crates/server/src/server/dashboard/api*.rs).

export type Currency = 'credits' | 'comp_points' | 'league_tokens';
export type Queue = 'tank' | 'damage' | 'support' | 'open' | 'ctf' | 'lucio';
export type PassPool = 'quick_play' | 'competitive';

export interface BoxCount {
  type: number;
  name: string;
  label: string;
  count: number;
}

/** The profile fields the page edits, plus its counts (`api.rs` `snapshot`). */
export type Profile = {
  player_name: string;
  region: string;
  game_region: string;
  level: number;
  credits: number;
  comp_points: number;
  league_tokens: number;
  endorsement_level: number;
  season: number;
  sms_protect: boolean;
  lobby_hero: string;
  events: string[];
  server_date: string;
  challenge: string;
  challenge_wins: number;
  unlock_all: boolean;
  bot_chat: boolean;
  stats: {boxes_opened?: number} | null;
  loot_boxes_count: number;
  unlocked_count: number;
  box_counts: BoxCount[];
  /** The chosen portrait frame's GUID; null: the level's own frame. */
  frame: string | null;
} & Record<`rating_${Queue}` | `matches_${Queue}` | `wins_${Queue}` | `passes_${PassPool}`, number>;

/** The fields `POST /api/update_profile` takes. */
export type ProfileChanges = Partial<Omit<Profile, 'stats' | 'loot_boxes_count' | 'unlocked_count' | 'box_counts' | 'frame'>>;

export interface AccountRow {
  name: string;
  online: boolean;
  selected: boolean;
  default: boolean;
}

export interface GameInstance {
  name: string;
  player: string;
  host: string;
  port: number;
  state: string;
}

export interface ModeRow {
  card: string;
  name: string;
  teams: string;
  competitive: boolean;
  players_to_start: number;
  fill_running: boolean;
}

export interface MapRow {
  guid: string;
  name: string;
}

export interface ServerInfo {
  host: string;
  port: number;
  connected_clients: number;
  uptime_seconds: number;
  game_instances: GameInstance[];
  game_runtime_available: boolean;
  second_games: boolean;
  matchmaking_supported: boolean;
  test_players: number;
  modes: ModeRow[];
  maps: MapRow[];
  /** The forced map's GUID, empty for each queue's own pick. */
  forced_map: string;
  /** The Workshop script file, empty for none. */
  workshop: string;
}

export interface Hero {
  guid: string;
  name: string;
}

export interface EventInfo {
  id: string;
  label: string;
  description: string;
  category: string;
  scene_status: string;
  scene_note: string;
}

export interface BoxType {
  id: number;
  name: string;
  label: string;
}

export interface Challenge {
  id: string;
  title: string;
  rewards: {name: string; type: string}[];
}

export interface Frame {
  level: number;
  guid: string;
}

export interface Catalogs {
  heroes: Hero[];
  npcs: string[];
  events: EventInfo[];
  box_types: BoxType[];
  challenges: Challenge[];
  frames: Frame[];
}

/** `GET /api/state`. */
export interface State {
  profile: Profile;
  battle_tag: string;
  accounts: AccountRow[];
  server: ServerInfo;
  catalogs: Catalogs;
}

export type CollectionKind = 'all' | 'skins' | 'weapons' | 'icons' | 'sprays' | 'emotes' | 'poses' | 'voicelines' | 'intros' | 'frames';

export interface CollectionQuery {
  account: string;
  kind?: CollectionKind;
  q?: string;
  hero?: string;
  currency?: Currency | '';
  owl?: boolean;
  page?: number;
}

/** An item (`frame` unset) or a portrait frame (`frame: true`). */
export interface CollectionItem {
  guid: string;
  name: string;
  hero: string;
  rarity: string;
  type: string;
  preview: string | null;
  owl?: boolean;
  owned?: boolean;
  removable?: boolean;
  price?: number | null;
  currency?: Currency | null;
  purchasable?: boolean;
  frame?: true;
  in_use?: boolean;
  chosen?: boolean;
}

/** `GET /api/collection`. */
export interface CollectionPage {
  items: CollectionItem[];
  page: number;
  total: number;
  pages: number;
}

export interface Ok {
  status: 'ok';
}

export interface Message {
  message: string;
}

export interface ProfileReply extends Ok {
  profile: Profile;
}

export interface Receipt {
  guid: string;
  price: number;
  currency: Currency;
  also: string[];
}

export type BotAction = 'list' | 'join' | 'invite' | 'remove';
