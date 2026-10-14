import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import {
  ApiError,
  BACKOFF_MS,
  HIDDEN_MS,
  Poller,
  TOKEN_KEY,
  VISIBLE_MS,
  makeClient,
  takeToken,
  type ConnState,
} from '../src/lib/api'

function memoryStorage() {
  const m = new Map<string, string>()
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
  }
}

describe('token', () => {
  it('takes it from the fragment, keeps it, and drops it from the address bar', () => {
    const storage = memoryStorage()
    const replaceState = vi.fn()
    const t = takeToken({ hash: '#token=abc123', pathname: '/', search: '' }, storage, {
      replaceState,
    })
    expect(t).toBe('abc123')
    expect(storage.getItem(TOKEN_KEY)).toBe('abc123')
    expect(replaceState).toHaveBeenCalledWith(null, '', '/')
    // A reload without the fragment still has it.
    expect(takeToken({ hash: '', pathname: '/', search: '' }, storage, { replaceState })).toBe(
      'abc123',
    )
  })

  it('is missing without a fragment or storage', () => {
    expect(
      takeToken({ hash: '', pathname: '/', search: '' }, memoryStorage(), {
        replaceState: vi.fn(),
      }),
    ).toBeNull()
  })

  it('is sent as a bearer header', async () => {
    const fetchFn = vi.fn(async () => new Response('{"ok":true}', { status: 200 }))
    await makeClient('tok', fetchFn as unknown as typeof fetch).get('/api/ping')
    const init = (fetchFn.mock.calls[0] as unknown as [string, RequestInit])[1]
    expect((init.headers as Record<string, string>).Authorization).toBe('Bearer tok')
  })

  it('maps a 409 to its message', async () => {
    const fetchFn = vi.fn(
      async () => new Response('{"error":"already resolved: x"}', { status: 409 }),
    )
    const r = await makeClient('tok', fetchFn as unknown as typeof fetch).act({ kind: 'x' })
    expect(r).toEqual({ ok: false, status: 409, error: 'already resolved: x' })
  })
})

describe('poller', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  function setup(versions: (number | Error)[], hidden = () => false) {
    const calls: number[] = []
    const states: ConnState[] = []
    let i = 0
    const p = new Poller({
      version: async () => {
        const v = versions[Math.min(i++, versions.length - 1)]
        if (v instanceof Error) throw v
        return v
      },
      onChange: (pos) => void calls.push(pos),
      onState: (s) => void states.push(s),
      hidden,
      now: () => 1000,
    })
    return { p, calls, states }
  }

  it('refetches only when the cursor moves', async () => {
    const { p, calls } = setup([1, 1, 1, 2, 2])
    p.start()
    await vi.advanceTimersByTimeAsync(VISIBLE_MS * 4 + 10)
    expect(calls).toEqual([1, 2])
    p.stop()
  })

  it('polls every 500 ms while visible and every 5 s while hidden', async () => {
    let hidden = false
    const { p } = setup([1], () => hidden)
    const spy = vi.fn(async () => 1)
    ;(p as unknown as { opts: { version: () => Promise<number> } }).opts.version = spy
    p.start()
    await vi.advanceTimersByTimeAsync(VISIBLE_MS * 3 + 10)
    expect(spy).toHaveBeenCalledTimes(4)
    hidden = true
    expect(p.delay()).toBe(HIDDEN_MS)
    await vi.advanceTimersByTimeAsync(VISIBLE_MS)
    const before = spy.mock.calls.length
    await vi.advanceTimersByTimeAsync(HIDDEN_MS - VISIBLE_MS - 20)
    expect(spy.mock.calls.length).toBe(before)
    p.stop()
  })

  it('backs off after errors and is disconnected after 2 failures', async () => {
    const down = new Error('connection refused')
    const { p, states } = setup([1, down, down, down, down, 3])
    p.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(states.at(-1)?.connected).toBe(true)
    await vi.advanceTimersByTimeAsync(VISIBLE_MS)
    expect(states.at(-1)?.connected).toBe(true)
    expect(p.delay()).toBe(BACKOFF_MS[0])
    await vi.advanceTimersByTimeAsync(BACKOFF_MS[0])
    expect(states.at(-1)?.connected).toBe(false)
    expect(p.delay()).toBe(BACKOFF_MS[1])
    await vi.advanceTimersByTimeAsync(BACKOFF_MS[1])
    expect(p.delay()).toBe(BACKOFF_MS[2])
    await vi.advanceTimersByTimeAsync(BACKOFF_MS[2])
    expect(p.delay()).toBe(BACKOFF_MS[2])
    await vi.advanceTimersByTimeAsync(BACKOFF_MS[2])
    expect(states.at(-1)?.connected).toBe(true)
    p.stop()
  })

  it('a refused token disconnects at once', async () => {
    const { p, states } = setup([new ApiError(401, 'no')])
    p.start()
    await vi.advanceTimersByTimeAsync(0)
    expect(states.at(-1)).toMatchObject({ connected: false, unauthorized: true })
    p.stop()
  })
})

describe('a server that stops answering', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('times out its polls and reads Disconnected, never stale green', async () => {
    // Accepts the connection but never answers (a hung or suspended process).
    const hung = ((_: string, init?: RequestInit) =>
      new Promise((_resolve, reject) =>
        init?.signal?.addEventListener('abort', () => reject(new Error('timed out'))),
      )) as unknown as typeof fetch
    const client = makeClient('tok', hung)
    let conn: ConnState = { connected: true, unauthorized: false, lastOk: null }
    const p = new Poller({
      version: async () =>
        (await client.get<{ last_event_pos: number }>('/api/version')).last_event_pos,
      onChange: () => {},
      onState: (s) => (conn = s),
      hidden: () => false,
    })
    p.start()
    await vi.advanceTimersByTimeAsync(20_000)
    p.stop()
    expect(conn.connected).toBe(false)
  })
})
