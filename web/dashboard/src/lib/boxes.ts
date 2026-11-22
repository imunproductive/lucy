import type {BoxType} from './types.ts';

// Each event's box colour, and its picture in the assets folder
// (`<assets>/boxes/<file>`, optional: without it the page draws a box).
const looks: [key: string, colour: string, file: string | null][] = [
  ['legendary anniversary', '#e8952f', 'legendary_anniv.png'],
  ['halloween terror', '#f07a1e', 'halloween.png'],
  ['winter wonderland', '#49b6e6', 'winter.png'],
  ['lunar new year', '#e0453f', 'lunar.png'],
  ['summer games', '#3fb64f', 'summer.png'],
  ['wrecking ball', '#e8952f', 'ham.png'],
  ['anniversary', '#b070e0', 'anniversary.png'],
  ['halloween', '#f07a1e', 'halloween.png'],
  ['legendary', '#e8952f', 'legendary.png'],
  ['standard', '#3d7bd6', 'standard.png'],
  ['archives', '#18b0a6', 'archives.png'],
  ['golden', '#e8b13a', 'golden.png'],
  ['ram', '#c58a4a', 'ham.png'],
];

function look(box: BoxType): [string, string, string | null] | undefined {
  const key = (box.label || box.name || '').toLowerCase().trim();
  return looks.find(([name]) => name === key) ?? looks.find(([name]) => key.includes(name));
}

export function boxColour(box: BoxType): string {
  return look(box)?.[1] ?? '#5b82a8';
}

export function boxPicture(box: BoxType): string | null {
  const file = look(box)?.[2];
  return file ? `/assets/boxes/${encodeURIComponent(file)}` : null;
}
