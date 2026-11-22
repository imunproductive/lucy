// Server-sent events from the dashboard (`GET /api/events`, stage 3): each
// event's data is JSON with the same payloads as the polled routes.
import type {ChatLine, LiveSnapshot} from './live.ts';

/** The events `/api/events` is to send, by event name. */
export interface DashboardEvents {
  live: LiveSnapshot;
  chat: {match: string; line: ChatLine};
}

export type StreamStatus = 'connecting' | 'open' | 'retrying' | 'closed';

/** The part of `EventSource` the helper uses (a stand-in in tests). */
export interface EventSourceLike {
  readonly readyState: number;
  onopen: ((event: Event) => void) | null;
  onerror: ((event: Event) => void) | null;
  addEventListener(type: string, listener: (event: MessageEvent) => void): void;
  close(): void;
}

export interface StreamOptions<E> {
  /** One handler per event name; its data arrives parsed. */
  on: {[K in keyof E]?: (data: E[K]) => void};
  status?(status: StreamStatus): void;
  /** Data that is not JSON. */
  invalid?(type: string, data: string): void;
  /** First wait before connecting again after the server closed the stream, ms. */
  retry?: number;
  /** Longest wait between attempts, ms. */
  maxRetry?: number;
  create?(url: string): EventSourceLike;
}

const CLOSED = 2;

/**
 * Listen to a server-sent event stream. The browser itself reconnects after a
 * dropped connection; when it gives up (the server refused the stream or
 * closed it for good), this connects again after a growing wait. Returns the
 * function that closes the stream.
 */
export function subscribe<E extends object = DashboardEvents>(url: string, options: StreamOptions<E>): () => void {
  const create = options.create ?? ((target: string) => new EventSource(target));
  const first = options.retry ?? 1000;
  const longest = options.maxRetry ?? 30000;
  let wait = first;
  let source: EventSourceLike | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let closed = false;

  const connect = () => {
    options.status?.('connecting');
    const current = create(url);
    source = current;
    current.onopen = () => {
      wait = first;
      options.status?.('open');
    };
    current.onerror = () => {
      if (closed || current.readyState !== CLOSED) return;
      current.close();
      options.status?.('retrying');
      timer = setTimeout(connect, wait);
      wait = Math.min(wait * 2, longest);
    };
    for (const type of Object.keys(options.on) as (keyof E & string)[]) {
      current.addEventListener(type, event => {
        let data: E[typeof type];
        try {
          data = JSON.parse(event.data);
        } catch {
          options.invalid?.(type, event.data);
          return;
        }
        options.on[type]?.(data);
      });
    }
  };

  connect();
  return () => {
    closed = true;
    clearTimeout(timer);
    source?.close();
    options.status?.('closed');
  };
}
