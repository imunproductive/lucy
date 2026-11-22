// The page's addresses. The server answers every path outside /api and
// /assets with index.html, so each of these can be loaded or bookmarked.

export type Route =
  | {name: 'profile'}
  | {name: 'events'}
  | {name: 'boxes'}
  | {name: 'collection'}
  | {name: 'server'}
  | {name: 'matches'}
  | {name: 'match'; id: string}
  | {name: 'not-found'};

export type PageName = Exclude<Route['name'], 'not-found'>;

export const paths: Record<Exclude<PageName, 'match'>, string> = {
  profile: '/',
  events: '/events',
  boxes: '/boxes',
  collection: '/collection',
  server: '/server',
  matches: '/matches',
};

export function matchPath(id: string): string {
  return `/matches/${encodeURIComponent(id)}`;
}

export function parseRoute(pathname: string): Route {
  const path = pathname.replace(/\/+$/, '') || '/';
  for (const [name, known] of Object.entries(paths)) {
    if (known === path) return {name} as Route;
  }
  const match = /^\/matches\/([^/]+)$/.exec(path);
  if (match) {
    try {
      return {name: 'match', id: decodeURIComponent(match[1])};
    } catch {
      return {name: 'not-found'};
    }
  }
  return {name: 'not-found'};
}

/** The old page's #hash addresses, for bookmarks made before the routes. */
const legacy: Record<string, string> = {
  '#overview': paths.profile,
  '#events': paths.events,
  '#boxes': paths.boxes,
  '#shop': paths.collection,
  '#sessions': paths.server,
};

export function legacyPath(hash: string): string | null {
  return legacy[hash] ?? null;
}
