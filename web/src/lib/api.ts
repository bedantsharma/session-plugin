// Talking to `bridge-screen ui`: the token from the URL fragment, a fetch wrapper that
// sends it, and the 500 ms version poller (spec §4.3, §6.4).

import type { OwnerAction } from './types'

export const TOKEN_KEY = 'bridge-screen-token'

/**
 * The token: taken from `#token=…` (then dropped from the address bar, so it isn't
 * bookmarked or shared) and kept in sessionStorage; else the one stored earlier.
 */
export function takeToken(
  loc: Pick<Location, 'hash' | 'pathname' | 'search'> = window.location,
  storage: Pick<Storage, 'getItem' | 'setItem'> = window.sessionStorage,
  hist: Pick<History, 'replaceState'> = window.history,
): string | null {
  const m = /(?:^#|&)token=([0-9a-f]+)/i.exec(loc.hash)
  if (m) {
    storage.setItem(TOKEN_KEY, m[1])
    hist.replaceState(null, '', loc.pathname + loc.search)
    return m[1]
  }
  return storage.getItem(TOKEN_KEY)
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message)
  }
}

export type ActResult = { ok: true; reply: string } | { ok: false; status: number; error: string }

export interface Client {
  get<T>(path: string): Promise<T>
  act(action: OwnerAction): Promise<ActResult>
}

/** A read that takes longer than this fails (and counts as a failed poll). */
export const GET_TIMEOUT_MS = 4000

export function makeClient(token: string | null, fetchFn: typeof fetch = fetch): Client {
  const headers = (): Record<string, string> => (token ? { Authorization: `Bearer ${token}` } : {})
  return {
    async get<T>(path: string): Promise<T> {
      // A server that accepts but never answers must not leave stale data looking live.
      const abort = new AbortController()
      const timer = setTimeout(() => abort.abort(), GET_TIMEOUT_MS)
      let res: Response
      try {
        res = await fetchFn(path, { headers: headers(), signal: abort.signal })
      } finally {
        clearTimeout(timer)
      }
      if (!res.ok) {
        let msg = res.statusText
        try {
          msg = ((await res.json()) as { error?: string }).error ?? msg
        } catch {
          // not JSON
        }
        throw new ApiError(res.status, msg)
      }
      return (await res.json()) as T
    },
    async act(action: OwnerAction): Promise<ActResult> {
      try {
        const res = await fetchFn('/api/action', {
          method: 'POST',
          headers: { ...headers(), 'Content-Type': 'application/json' },
          body: JSON.stringify(action),
        })
        const body = (await res.json().catch(() => ({}))) as { reply?: string; error?: string }
        if (res.ok) return { ok: true, reply: body.reply ?? '' }
        return { ok: false, status: res.status, error: body.error ?? res.statusText }
      } catch (e) {
        return { ok: false, status: 0, error: `Disconnected: ${(e as Error).message}` }
      }
    },
  }
}

export interface ConnState {
  /** False after 2 failed polls in a row, or once the token is refused. */
  connected: boolean
  /** The token was refused (401). */
  unauthorized: boolean
  /** When a poll last succeeded (ms since the epoch). */
  lastOk: number | null
}

export const VISIBLE_MS = 500
export const HIDDEN_MS = 5000
export const BACKOFF_MS = [1000, 2000, 5000]
/** Failed polls in a row before the screen says Disconnected. */
export const FAILS_BEFORE_DISCONNECTED = 2

export interface PollerOptions {
  /** The current cursor (`/api/version`). */
  version: () => Promise<number>
  /** Called when the cursor moves (and on the first successful poll). */
  onChange: (pos: number) => void | Promise<void>
  onState: (s: ConnState) => void
  hidden?: () => boolean
  now?: () => number
}

/**
 * Polls the cursor every 500 ms while the tab is visible (5 s while hidden), refetches
 * only when it moves, and backs off (1 s, 2 s, 5 s) after errors.
 */
export class Poller {
  private timer: ReturnType<typeof setTimeout> | null = null
  private cursor: number | null = null
  private failures = 0
  private stopped = true
  state: ConnState = { connected: true, unauthorized: false, lastOk: null }

  constructor(private opts: PollerOptions) {}

  private hidden(): boolean {
    return this.opts.hidden
      ? this.opts.hidden()
      : typeof document !== 'undefined' && document.hidden
  }

  private set(s: Partial<ConnState>) {
    this.state = { ...this.state, ...s }
    this.opts.onState(this.state)
  }

  /** The wait before the next poll. */
  delay(): number {
    if (this.failures > 0) return BACKOFF_MS[Math.min(this.failures, BACKOFF_MS.length) - 1]
    return this.hidden() ? HIDDEN_MS : VISIBLE_MS
  }

  start() {
    this.stopped = false
    void this.tick()
  }

  stop() {
    this.stopped = true
    if (this.timer) clearTimeout(this.timer)
    this.timer = null
  }

  /** Poll now (e.g. when the tab becomes visible again). */
  wake() {
    if (this.stopped) return
    if (this.timer) clearTimeout(this.timer)
    void this.tick()
  }

  async tick() {
    this.timer = null
    try {
      const pos = await this.opts.version()
      this.failures = 0
      const moved = pos !== this.cursor
      this.cursor = pos
      if (moved) await this.opts.onChange(pos)
      this.set({ connected: true, unauthorized: false, lastOk: (this.opts.now ?? Date.now)() })
    } catch (e) {
      this.failures += 1
      const unauthorized = e instanceof ApiError && e.status === 401
      this.set({
        unauthorized,
        connected: !unauthorized && this.failures < FAILS_BEFORE_DISCONNECTED,
      })
    }
    if (!this.stopped) this.timer = setTimeout(() => void this.tick(), this.delay())
  }
}
