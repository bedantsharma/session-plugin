// Anything that moves stops within 5 s (WCAG 2.2.2); under prefers-reduced-motion,
// a single fade at most.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const root = resolve(process.cwd(), 'src')

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f)
    if (statSync(p).isDirectory()) return p.includes('components/ui') ? [] : files(p)
    return /\.(css|svelte)$/.test(f) ? [p] : []
  })
}

function seconds(v: string): number {
  return v.endsWith('ms') ? parseFloat(v) / 1000 : parseFloat(v)
}

/** Every `animation:` shorthand as (total seconds, iterations). */
function animations(css: string): { total: number; count: string }[] {
  return [...css.matchAll(/animation:\s*([^;]+);/g)].map((m) => {
    const parts = m[1].trim().split(/\s+/)
    const dur = parts.find((p) => /^[\d.]+m?s$/.test(p)) ?? '0s'
    const count = parts.find((p) => /^(\d+|infinite)$/.test(p)) ?? '1'
    const n = count === 'infinite' ? Infinity : Number(count)
    return { total: seconds(dur) * n, count }
  })
}

describe('motion', () => {
  const css = files(root).map((f) => readFileSync(f, 'utf8'))

  it('no animation runs 5 s or longer', () => {
    const all = css.flatMap(animations)
    expect(all.length).toBeGreaterThan(0)
    for (const a of all) expect(a.total).toBeLessThan(5)
  })

  it('reduced motion gets a single fade at most', () => {
    for (const text of css) {
      for (const m of text.matchAll(
        /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/g,
      )) {
        for (const a of animations(m[1])) {
          expect(a.count).toBe('1')
        }
        expect(m[1]).not.toMatch(/pulse-ring/)
      }
    }
  })
})
