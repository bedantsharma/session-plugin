// Theme rules beyond the token contrast: no web fonts (the card is one self-contained
// file and the palette uses system fonts), no surface painted with the muted *text* grey,
// and under reduced motion only colour and opacity change.
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join, resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const src = resolve(process.cwd(), 'src')
const css = readFileSync(join(src, 'app.css'), 'utf8')

function files(dir: string): string[] {
  return readdirSync(dir).flatMap((f) => {
    const p = join(dir, f)
    if (statSync(p).isDirectory()) return files(p)
    return /\.(css|svelte|ts)$/.test(f) ? [p] : []
  })
}

describe('theme', () => {
  it('loads no web fonts', () => {
    expect(css).not.toMatch(/@fontsource|@font-face|url\(/)
    expect(css).toMatch(/--font-sans:\s*system-ui/)
  })

  it('never paints a surface with the muted text grey', () => {
    // `--muted` is a text colour here (shadcn's `muted` surface is `--line`).
    for (const f of files(src)) {
      const hits = readFileSync(f, 'utf8').match(/\bbg-muted(?!-)/g)
      expect(hits, f).toBeNull()
    }
  })

  it('under reduced motion, stops animations and keeps only colour/opacity transitions', () => {
    const block = /@media \(prefers-reduced-motion: reduce\) \{([\s\S]*?)\n\}/.exec(css)
    expect(block).not.toBeNull()
    expect(block![1]).toMatch(/animation:\s*none !important/)
    expect(block![1]).toMatch(
      /transition-property:\s*color, background-color, border-color, outline-color, opacity !important/,
    )
  })
})
