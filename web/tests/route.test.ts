import { describe, expect, it, vi } from 'vitest'
import { takeToken } from '../src/lib/api'
import { fromHash, makeNav, toHash, type Screen } from '../src/lib/route'

const screens: Screen[] = [
  { kind: 'glance' },
  { kind: 'project', repo: 'abc123' },
  { kind: 'session', id: 's1-uuid' },
  { kind: 'task', repo: 'abc123', id: 'T2-1.3' },
  { kind: 'history', repo: 'abc123' },
  { kind: 'tuning', repo: 'abc123' },
  { kind: 'calibration', repo: 'abc123' },
  { kind: 'pages', repo: 'abc123' },
]

function fakeBrowser(hash = '') {
  const loc = { hash, pathname: '/', search: '' }
  const stack: { hash: string; state: unknown }[] = [{ hash, state: null }]
  let at = 0
  const listeners: (() => void)[] = []
  const hist = {
    get state() {
      return stack[at].state
    },
    pushState: vi.fn((state: unknown, _t: string, url: string) => {
      stack.splice(at + 1)
      stack.push({ hash: url.slice(url.indexOf('#')), state })
      at += 1
      loc.hash = stack[at].hash
    }),
    replaceState: vi.fn((state: unknown, _t: string, url: string) => {
      const i = url.indexOf('#')
      stack[at] = { hash: i >= 0 ? url.slice(i) : '', state }
      loc.hash = stack[at].hash
    }),
    back: vi.fn(() => {
      if (at > 0) {
        at -= 1
        loc.hash = stack[at].hash
        listeners.forEach((l) => l())
      }
    }),
  }
  const target = {
    addEventListener: (_: string, l: () => void) => void listeners.push(l),
    removeEventListener: (_: string, l: () => void) =>
      void listeners.splice(listeners.indexOf(l), 1),
  }
  return { loc, hist, target: target as unknown as Window }
}

describe('hash routes', () => {
  it('route_roundtrip_all_screens', () => {
    for (const s of screens) expect(fromHash(toHash(s))).toEqual(s)
  })

  it('fromHash_rejects_garbage_to_glance', () => {
    for (const h of [
      '',
      '#',
      '#/',
      '#/nope/x',
      '#/project',
      '#/task/onlyrepo',
      '#/task//',
      '#%E0%A4%A',
    ]) {
      expect(fromHash(h)).toEqual({ kind: 'glance' })
    }
    // A token riding along is ignored.
    expect(fromHash('#/project/R&token=abc')).toEqual({ kind: 'project', repo: 'R' })
  })

  it('go_does_not_push_twice', () => {
    const b = fakeBrowser()
    const nav = makeNav(b.loc, b.hist as unknown as History, b.target)
    nav.go({ kind: 'project', repo: 'R' })
    nav.go({ kind: 'project', repo: 'R' })
    expect(b.hist.pushState).toHaveBeenCalledTimes(1)
    nav.go({ kind: 'task', repo: 'R', id: '1.1' })
    expect(b.hist.pushState).toHaveBeenCalledTimes(2)
  })

  it('Back restores the screen and the header button uses history.back when it can', () => {
    const b = fakeBrowser()
    const nav = makeNav(b.loc, b.hist as unknown as History, b.target)
    const seen: Screen[] = []
    nav.listen((s) => seen.push(s))
    nav.go({ kind: 'project', repo: 'R' })
    nav.go({ kind: 'task', repo: 'R', id: '1.1' })
    expect(nav.back()).toBe(true)
    expect(seen.at(-1)).toEqual({ kind: 'project', repo: 'R' })
    expect(nav.back()).toBe(true)
    expect(seen.at(-1)).toEqual({ kind: 'glance' })
    // Nothing of the dashboard's own before: the glance replaces the entry.
    expect(nav.back()).toBe(false)
    expect(b.loc.hash).toBe('#/')
  })

  it('a reload on a drill screen opens it from the hash', () => {
    const b = fakeBrowser('#/project/R')
    const nav = makeNav(b.loc, b.hist as unknown as History, b.target)
    expect(nav.current()).toEqual({ kind: 'project', repo: 'R' })
  })
})

describe('token with routes', () => {
  const storage = () => {
    const m = new Map<string, string>()
    return {
      getItem: (k: string) => m.get(k) ?? null,
      setItem: (k: string, v: string) => void m.set(k, v),
    }
  }

  it('takeToken_keeps_route', () => {
    const replaceState = vi.fn()
    const t = takeToken(
      { hash: '#/project/R&token=abc123', pathname: '/', search: '' },
      storage(),
      {
        replaceState,
      },
    )
    expect(t).toBe('abc123')
    expect(replaceState).toHaveBeenCalledWith(null, '', '/#/project/R')
  })

  it('takeToken_plain_token_still_works', () => {
    const replaceState = vi.fn()
    const t = takeToken({ hash: '#token=abc123', pathname: '/', search: '' }, storage(), {
      replaceState,
    })
    expect(t).toBe('abc123')
    expect(replaceState).toHaveBeenCalledWith(null, '', '/')
  })
})
