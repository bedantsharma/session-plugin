// Hash routes for the dashboard's eight screens (spec 2026-11-14 §5), so the browser's Back
// button and the two-finger swipe move between them instead of leaving the page. Hashes,
// not server routes: the single embedded page and the token flow stay as they are.

export type Screen =
  | { kind: 'glance' }
  | { kind: 'project'; repo: string }
  | { kind: 'session'; id: string }
  | { kind: 'task'; repo: string; id: string }
  | { kind: 'history'; repo: string }
  | { kind: 'tuning'; repo: string }
  | { kind: 'calibration'; repo: string }
  | { kind: 'pages'; repo: string }

const enc = encodeURIComponent

/** The hash that names a screen (`#/` for the glance). */
export function toHash(s: Screen): string {
  switch (s.kind) {
    case 'glance':
      return '#/'
    case 'session':
      return `#/session/${enc(s.id)}`
    case 'task':
      return `#/task/${enc(s.repo)}/${enc(s.id)}`
    default:
      return `#/${s.kind}/${enc(s.repo)}`
  }
}

function dec(part: string | undefined): string | null {
  if (!part) return null
  try {
    return decodeURIComponent(part)
  } catch {
    return null
  }
}

/** The screen a hash names; anything unreadable is the glance. */
export function fromHash(hash: string): Screen {
  const path = hash.replace(/^#/, '').split('&')[0]
  const [, kind, a, b] = path.split('/')
  const first = dec(a)
  switch (kind) {
    case 'project':
    case 'history':
    case 'tuning':
    case 'calibration':
    case 'pages':
      return first ? ({ kind, repo: first } as Screen) : { kind: 'glance' }
    case 'session':
      return first ? { kind: 'session', id: first } : { kind: 'glance' }
    case 'task': {
      const id = dec(b)
      return first && id ? { kind: 'task', repo: first, id } : { kind: 'glance' }
    }
    default:
      return { kind: 'glance' }
  }
}

/**
 * The navigation helper `App` uses: `go` pushes a history entry (never twice for the same
 * screen) and counts entries of the dashboard's own, so "← Glance" can use `history.back()`
 * when the previous entry is the glance; `restore` reads the screen back after
 * Back/forward.
 */
export interface Nav {
  go(next: Screen): void
  /**
   * "← Glance": `true` when the entry before this one is the glance and it went back
   * through the browser's history (a popstate follows); `false` when it replaced this
   * entry with the glance (the caller shows it), so Back from the glance never lands on a
   * drill screen.
   */
  toGlance(): boolean
  /** The screen the current hash names. */
  current(): Screen
  /** The page asked for is gone: replace the entry with the glance. */
  replaceWithGlance(): void
  listen(onScreen: (s: Screen) => void): () => void
}

type Loc = Pick<Location, 'hash' | 'pathname' | 'search'>
type Hist = Pick<History, 'pushState' | 'replaceState' | 'back' | 'state'>

export function makeNav(
  loc: Loc = window.location,
  hist: Hist = window.history,
  target: Pick<Window, 'addEventListener' | 'removeEventListener'> = window,
): Nav {
  const base = () => loc.pathname + loc.search
  const index = () => ((hist.state as { bs?: number } | null)?.bs ?? 0) as number
  const fromGlance = () => (hist.state as { fromGlance?: boolean } | null)?.fromGlance === true
  // The first screen is whatever the hash says; mark the entry as the dashboard's first.
  if ((hist.state as { bs?: number } | null)?.bs === undefined)
    hist.replaceState({ bs: 0 }, '', base() + loc.hash)
  return {
    current: () => fromHash(loc.hash),
    replaceWithGlance() {
      hist.replaceState({ bs: index() }, '', base() + toHash({ kind: 'glance' }))
    },
    go(next) {
      const here = fromHash(loc.hash)
      if (toHash(next) === toHash(here)) return
      hist.pushState(
        { bs: index() + 1, fromGlance: here.kind === 'glance' },
        '',
        base() + toHash(next),
      )
    },
    toGlance() {
      if (fromGlance()) {
        hist.back()
        return true
      }
      hist.replaceState({ bs: index() }, '', base() + toHash({ kind: 'glance' }))
      return false
    },
    listen(onScreen) {
      const handler = () => onScreen(fromHash(loc.hash))
      target.addEventListener('popstate', handler)
      return () => target.removeEventListener('popstate', handler)
    },
  }
}
