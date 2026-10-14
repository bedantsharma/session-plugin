// WCAG contrast for every status/bar/text token pair, in both themes, read from app.css:
// ≥ 3:1 for graphics and control edges (SC 1.4.11), ≥ 4.5:1 for text (SC 1.4.3).
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it } from 'vitest'

const css = readFileSync(resolve(process.cwd(), 'src/app.css'), 'utf8')

function theme(name: string): Record<string, string> {
  const block = new RegExp(`/\\* theme:${name} \\*/([\\s\\S]*?)/\\* end \\*/`).exec(css)
  if (!block) throw new Error(`no theme:${name} block in app.css`)
  const out: Record<string, string> = {}
  for (const m of block[1].matchAll(/--([\w-]+):\s*(#[0-9a-fA-F]{6})/g)) out[m[1]] = m[2]
  return out
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const v = parseInt(hex.slice(i, i + 2), 16) / 255
    return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4
  })
  return 0.2126 * r + 0.7152 * g + 0.0722 * b
}

export function ratio(a: string, b: string): number {
  const [x, y] = [luminance(a), luminance(b)].sort((p, q) => q - p)
  return (x + 0.05) / (y + 0.05)
}

describe.each(['dark', 'light'])('%s theme', (name) => {
  const t = theme(name)
  const backgrounds = ['bg', 'surface']

  it.each(['fg', 'muted', 'clay', 'ok', 'warn', 'bad'])('text %s ≥ 4.5:1', (fg) => {
    for (const bg of backgrounds)
      expect(ratio(t[fg], t[bg]), `${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5)
  })

  it.each(['clay', 'ok', 'warn', 'bad', 'edge'])('graphic %s ≥ 3:1', (g) => {
    for (const bg of backgrounds)
      expect(ratio(t[g], t[bg]), `${g} on ${bg}`).toBeGreaterThanOrEqual(3)
  })

  it.each(['fg', 'bad'])('text %s on a hovered control (line) ≥ 4.5:1', (fg) => {
    expect(ratio(t[fg], t.line)).toBeGreaterThanOrEqual(4.5)
  })

  it('primary buttons (bg on fg) keep their text readable', () => {
    expect(ratio(t.bg, t.fg)).toBeGreaterThanOrEqual(4.5)
  })

  it('filled red buttons keep their text readable', () => {
    expect(ratio(t['on-accent'], t.bad)).toBeGreaterThanOrEqual(4.5)
  })
})

it('the desktop card light override matches the light theme', () => {
  const light = theme('light')
  const card = /:root\[data-theme='light'\] \{([\s\S]*?)\}/.exec(css)![1]
  for (const [k, v] of Object.entries(light)) expect(card).toContain(`--${k}: ${v}`)
})
