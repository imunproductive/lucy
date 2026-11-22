// Client-side navigation with the History API: links to the page's own
// paths change the view without a reload.
import {legacyPath, parseRoute} from './routes.ts';
import type {Route} from './routes.ts';

class Router {
  path = $state(location.pathname);
  route: Route = $derived(parseRoute(this.path));

  go(path: string, replace = false): void {
    if (path !== location.pathname) {
      if (replace) history.replaceState(null, '', path);
      else history.pushState(null, '', path);
    }
    this.path = location.pathname;
    if (!replace) window.scrollTo({top: 0});
  }

  start(): void {
    const legacy = legacyPath(location.hash);
    if (legacy) this.go(legacy, true);
    window.addEventListener('popstate', () => {
      this.path = location.pathname;
    });
    document.addEventListener('click', event => {
      if (event.defaultPrevented || event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
      const link = (event.target as Element | null)?.closest?.('a');
      if (!link || link.target || link.hasAttribute('download') || link.origin !== location.origin) return;
      if (/^\/(api|assets)\//.test(link.pathname) || (link.hash && link.pathname === location.pathname)) return;
      event.preventDefault();
      this.go(link.pathname);
    });
  }
}

export const router = new Router();
