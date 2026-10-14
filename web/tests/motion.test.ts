// Motion gating (plan §5, WCAG 2.2.2): every looping animation runs only with no
// reduced-motion preference AND the header's Motion switch on; one-off cues stay short;
// under reduced motion nothing loops or travels.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { afterEach, describe, expect, it, vi } from 'vitest'

const root = resolve(process.cwd(), 'src')
const appCss = readFileSync(join(root, 'app.css'), 'utf8')

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f)
    if (statSync(p).isDirectory()) return p.includes('components/ui') ? [] : files(p)
    return /\.(css|svelte)$/.test(f) ? [p] : []
  })
}

/** The bodies of every `@media <query> { … }` block (brace-matched), and the text without them. */
function media(css: string, query: string): { blocks: string[]; rest: string } {
  const blocks: string[] = []
  let rest = css
  for (;;) {
    const at = rest.indexOf(`@media ${query}`)
    if (at < 0) break
    const open = rest.indexOf('{', at)
    let depth = 0
    let end = open
    for (; end < rest.length; end++) {
      if (rest[end] === '{') depth++
      else if (rest[end] === '}' && --depth === 0) break
    }
    blocks.push(rest.slice(open + 1, end))
    rest = rest.slice(0, at) + rest.slice(end + 1)
  }
  return { blocks, rest }
}

function seconds(v: string): number {
  return v.endsWith('ms') ? parseFloat(v) / 1000 : parseFloat(v)
}

/** Every `animation:` shorthand as (keyframes name, total seconds, iterations). */
function animations(css: string) {
  return [...css.matchAll(/animation:\s*([^;]+);/g)].map((m) => {
    const parts = m[1].replace('!important', '').trim().split(/\s+/)
    const dur = parts.find((p) => /^[\d.]+m?s$/.test(p)) ?? '0s'
    const count = parts.find((p) => /^(\d+|infinite)$/.test(p)) ?? '1'
    const n = count === 'infinite' ? Infinity : Number(count)
    return { name: parts[0], total: seconds(dur) * n, count }
  })
}

const GATE = ":root:not([data-motion='off'])"
const NO_PREF = '(prefers-reduced-motion: no-preference)'
const REDUCE = '(prefers-reduced-motion: reduce)'
const LOOPS = ['ring', 'breathe', 'sweep', 'spin']

describe('motion gating', () => {
  const gated = media(appCss, NO_PREF)

  it('has the gated block, and every rule in it needs the Motion switch on', () => {
    expect(gated.blocks.length).toBeGreaterThan(0)
    for (const block of gated.blocks)
      for (const m of block.matchAll(/([^{}]+)\{[^{}]*\}/g)) {
        const selector = m[1].replace(/\/\*[\s\S]*?\*\//g, '').trim()
        for (const sel of selector.split(','))
          expect(sel.trim().startsWith(GATE), `${sel.trim()} is not gated`).toBe(true)
      }
  })

  it('runs every looping animation (ring, breathe, sweep, spin) inside it', () => {
    const inside = gated.blocks.join('\n')
    for (const name of LOOPS) expect(inside, name).toMatch(new RegExp(`animation:\\s*${name}\\b`))
    expect(animations(inside).some((a) => a.count === 'infinite')).toBe(true)
  })

  it('nothing outside it loops or lasts 5 s or longer', () => {
    const outside = [
      gated.rest,
      ...files(root)
        .filter((f) => !f.endsWith('app.css'))
        .map((f) => readFileSync(f, 'utf8')),
    ]
    const all = outside.flatMap(animations)
    expect(all.length).toBeGreaterThan(0)
    for (const a of all) {
      expect(a.count, `${a.name} loops outside the gate`).toBe('1')
      expect(a.total, a.name).toBeLessThan(5)
      expect(LOOPS).not.toContain(a.name)
    }
  })

  it('under reduced motion: everything stops, then only single fades come back', () => {
    const { blocks } = media(appCss, REDUCE)
    expect(blocks.length).toBe(1)
    expect(blocks[0]).toMatch(/animation:\s*none !important/)
    // Colour changes (hover) stay; movement does not.
    expect(blocks[0]).toMatch(/transition-property:\s*color, background-color/)
    for (const a of animations(blocks[0])) {
      if (a.name === 'none') continue
      expect(a.count).toBe('1')
      expect(['fade-in', 'beat-fade', 'flash']).toContain(a.name)
    }
  })

  it('the bar-width transition only runs behind the gate', () => {
    expect(gated.rest).not.toMatch(/transition:\s*width/)
    expect(gated.blocks.join('\n')).toMatch(/transition:\s*width 600ms/)
  })
})

describe('the Motion setting', () => {
  afterEach(() => {
    vi.resetModules()
    localStorage.clear()
    vi.unstubAllGlobals()
  })

  function stubReduced(reduce: boolean) {
    vi.stubGlobal(
      'matchMedia',
      (q: string) => ({ matches: reduce && q.includes('reduce') }) as MediaQueryList,
    )
  }

  it('is on by default and sets data-motion on <html>', async () => {
    stubReduced(false)
    const { motion } = await import('../src/lib/motion.svelte')
    expect(motion.on).toBe(true)
    expect(document.documentElement.dataset.motion).toBe('on')
  })

  it('starts off under reduced motion', async () => {
    stubReduced(true)
    const { motion } = await import('../src/lib/motion.svelte')
    expect(motion.on).toBe(false)
    expect(document.documentElement.dataset.motion).toBe('off')
  })

  it('remembers the choice', async () => {
    stubReduced(false)
    const first = (await import('../src/lib/motion.svelte')).motion
    first.set(false)
    expect(localStorage.getItem('bridge-screen/motion')).toBe('off')
    vi.resetModules()
    const again = (await import('../src/lib/motion.svelte')).motion
    expect(again.on).toBe(false)
  })

  it('ticks live timers each second, or each minute with motion off', async () => {
    vi.useFakeTimers()
    try {
      stubReduced(false)
      const { motion } = await import('../src/lib/motion.svelte')
      const t0 = motion.now
      vi.advanceTimersByTime(1000)
      expect(motion.now).toBeGreaterThan(t0)
      motion.set(false)
      const t1 = motion.now
      vi.advanceTimersByTime(30_000)
      expect(motion.now).toBe(t1)
      vi.advanceTimersByTime(30_000)
      expect(motion.now).toBeGreaterThan(t1)
    } finally {
      vi.useRealTimers()
    }
  })
})
