import type {Currency, Frame} from './types.ts';

const numbers = new Intl.NumberFormat('en-US');

export function number(value: unknown): string {
  return numbers.format(Number(value) || 0);
}

/** "2 h 5 min", "3 min 10 s", "42 s". */
export function duration(totalSeconds: number): string {
  let seconds = Math.max(0, Math.floor(Number(totalSeconds) || 0));
  const hours = Math.floor(seconds / 3600);
  seconds %= 3600;
  const minutes = Math.floor(seconds / 60);
  if (hours) return `${hours} h ${minutes} min`;
  if (minutes) return `${minutes} min ${seconds % 60} s`;
  return `${seconds} s`;
}

export const currencies: Record<Currency, {label: string; unit: string; symbol: string}> = {
  credits: {label: 'Credits', unit: 'credits', symbol: 'C'},
  comp_points: {label: 'Competitive points', unit: 'competitive points', symbol: 'CP'},
  league_tokens: {label: 'League tokens', unit: 'league tokens', symbol: 'LT'},
};

export function price(amount: number | null | undefined, currency: Currency | null | undefined): string {
  return `${number(amount)} ${currency ? currencies[currency].unit : ''}`.trim();
}

export const itemTypes: Record<string, string> = {
  Skin: 'Skin',
  WeaponSkin: 'Weapon skin',
  Icon: 'Player icon',
  Spray: 'Spray',
  Emote: 'Emote',
  VictoryPose: 'Victory pose',
  VoiceLine: 'Voice line',
  HighlightIntro: 'Highlight intro',
  PortraitFrame: 'Portrait frame',
};

// Level rules: the card shows a level from 1 to 100; every 100 levels add a
// star (up to 5), and every 600 levels move up a tier (Bronze to Diamond).
// The total level is what the server keeps.
export const FRAME_TIERS = ['Bronze', 'Silver', 'Gold', 'Platinum', 'Diamond'] as const;
export const MAX_LEVEL = 3000;

export interface LevelParts {
  tier: number;
  stars: number;
  shown: number;
}

export function levelParts(total: number): LevelParts {
  const done = Math.min(Math.max(1, Math.floor(total) || 1), MAX_LEVEL) - 1;
  return {tier: Math.floor(done / 600), stars: Math.floor((done % 600) / 100), shown: (done % 100) + 1};
}

export function totalLevel(parts: LevelParts): number {
  const shown = Math.min(100, Math.max(1, Math.floor(parts.shown) || 1));
  return parts.tier * 600 + parts.stars * 100 + shown;
}

/** "Gold ★★" for the frame a level reaches. */
export function frameName(level: number): string {
  const parts = levelParts(level);
  return `${FRAME_TIERS[parts.tier]}${parts.stars ? ` ${'★'.repeat(parts.stars)}` : ''}`;
}

/** The highest frame the level unlocks; the table's GUIDs are not in level order. */
export function frameForLevel(frames: Frame[], total: number): Frame | null {
  let found: Frame | null = null;
  for (const frame of frames) if (frame.level <= total && (!found || frame.level >= found.level)) found = frame;
  return found;
}

/** The picture the server has for an item or frame GUID ("0x02500000000011AB"). */
export function previewUrl(guid: string): string {
  return `/assets/previews/${guid.replace(/^0x/i, '').toUpperCase().padStart(16, '0')}.webp`;
}

/** Two letters for an item without a picture. */
export function initials(text: string): string {
  return (
    text
      .replace(/[^\p{L}\p{N} ]/gu, '')
      .split(/\s+/)
      .filter(Boolean)
      .map(word => word[0])
      .join('')
      .slice(0, 2)
      .toUpperCase() || '?'
  );
}

export function clock(date: Date): string {
  return date.toLocaleTimeString('en-US', {hour: '2-digit', minute: '2-digit', second: '2-digit'});
}
