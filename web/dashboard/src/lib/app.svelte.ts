// The page's shared state: the account it shows, the server's last answer
// for it, the connection and the toasts.
import * as api from './api.ts';
import type {State} from './types.ts';

export interface Toast {
  id: number;
  text: string;
  error: boolean;
}

/** How often the page reads the server's state while it is shown, ms. */
const POLL_INTERVAL = 10_000;

class App {
  /** `GET /api/state` for `account`; null while it loads. */
  state = $state<State | null>(null);
  /** The account the page shows and changes; empty until the first answer names the selected one. */
  account = $state('');
  /** Why the last refresh failed; empty when it worked. */
  error = $state('');
  /** null before the first answer. */
  connected = $state<boolean | null>(null);
  updated = $state<Date | null>(null);
  refreshing = $state(false);
  /** Bumped by each refresh, for the views that read more than the state (the collection). */
  revision = $state(0);
  toasts = $state<Toast[]>([]);

  #request = 0;
  #holds = 0;
  #toastId = 0;

  /**
   * Pause the polling while a form saves or a dialog is open, so a refresh
   * does not change what the user is looking at. Returns the release.
   */
  hold(): () => void {
    this.#holds += 1;
    let held = true;
    return () => {
      if (held) this.#holds -= 1;
      held = false;
    };
  }

  async refresh(): Promise<void> {
    const request = ++this.#request;
    const account = this.account;
    this.refreshing = true;
    try {
      const state = await api.getState(account || undefined);
      if (request !== this.#request || account !== this.account) return;
      this.state = state;
      if (!this.account) {
        this.account = state.accounts.find(row => row.selected)?.name ?? state.profile.player_name;
      }
      this.error = '';
      this.connected = true;
      this.updated = new Date();
      this.revision += 1;
    } catch (error) {
      if (request !== this.#request) return;
      this.error = error instanceof Error ? error.message : String(error);
      this.connected = false;
    } finally {
      if (request === this.#request) this.refreshing = false;
    }
  }

  /** Show another account: the server's selected account for the next login changes too. */
  async switchAccount(name: string): Promise<void> {
    await api.selectAccount(name);
    await this.show(name);
  }

  /** Show another account (already selected on the server). */
  async show(name: string): Promise<void> {
    if (name === this.account) return;
    this.#request += 1;
    this.account = name;
    this.state = null;
    await this.refresh();
  }

  toast(text: string, error = false): void {
    const id = ++this.#toastId;
    this.toasts.push({id, text, error});
    setTimeout(() => this.dismiss(id), error ? 10_000 : 6_500);
  }

  /** Toast an action's failure. */
  fail(error: unknown): void {
    this.toast(error instanceof Error ? error.message : String(error), true);
  }

  dismiss(id: number): void {
    this.toasts = this.toasts.filter(toast => toast.id !== id);
  }

  /** The first read, then one every POLL_INTERVAL while the page is visible and nothing holds it. */
  start(): void {
    void this.refresh();
    setInterval(() => {
      if (!document.hidden && this.#holds === 0) void this.refresh();
    }, POLL_INTERVAL);
    document.addEventListener('visibilitychange', () => {
      if (!document.hidden) void this.refresh();
    });
  }
}

export const app = new App();
