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

// color-mix(in oklab, a p%, b), as the browser computes it, back to sRGB hex.
const lin = (v: number) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4)
const gam = (v: number) => (v <= 0.0031308 ? 12.92 * v : 1.055 * v ** (1 / 2.4) - 0.055)

function toOklab(hex: string): number[] {
  const [r, g, b] = [1, 3, 5].map((i) => lin(parseInt(hex.slice(i, i + 2), 16) / 255))
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b)
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b)
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b)
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ]
}

function fromOklab([L, a, b]: number[]): string {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3
  return (
    '#' +
    [
      4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
      -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
      -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
    ]
      .map((v) =>
        Math.round(Math.min(1, Math.max(0, gam(v))) * 255)
          .toString(16)
          .padStart(2, '0'),
      )
      .join('')
  )
}

export function mix(a: string, b: string, pct: number): string {
  const [x, y] = [toOklab(a), toOklab(b)]
  return fromOklab(x.map((v, i) => v * (pct / 100) + y[i] * (1 - pct / 100)))
}

/** The tints block: name → [token, %, base token]. */
function tints(): Record<string, [string, number, string]> {
  const block = /\/\* tints \*\/([\s\S]*?)\/\* end \*\//.exec(css)
  if (!block) throw new Error('no tints block in app.css')
  const out: Record<string, [string, number, string]> = {}
  for (const m of block[1].matchAll(
    /--([\w-]+):\s*color-mix\(in oklab, var\(--([\w-]+)\) (\d+)%, var\(--([\w-]+)\)\)/g,
  ))
    out[m[1]] = [m[2], Number(m[3]), m[4]]
  return out
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

  // Every text colour drawn on a tint (chips, the pill, hovered and working rows, the
  // change flash) keeps 4.5:1.
  const tint = (name: string) => {
    const [token, pct, base] = tints()[name]
    return mix(t[token], t[base], pct)
  }
  it.each([
    ['bad', 'tint-bad'],
    ['muted', 'tint-bad'],
    ['fg', 'tint-bad'],
    ['ok', 'tint-ok'],
    ['warn', 'tint-warn'],
    ['fg', 'hover'],
    ['muted', 'hover'],
    ['fg', 'tint-working'],
    ['muted', 'tint-working'],
    ['ok', 'tint-working'],
    ['warn', 'tint-working'],
    ['fg', 'tint-flash'],
    ['muted', 'tint-flash'],
  ])('text %s on %s ≥ 4.5:1', (fg, bg) => {
    expect(ratio(t[fg], tint(bg)), `${fg} on ${bg}`).toBeGreaterThanOrEqual(4.5)
  })

  it('progress fill and the claimed hatch stripes stand out from the track (line) ≥ 3:1', () => {
    expect(ratio(t.clay, t.line)).toBeGreaterThanOrEqual(3)
  })
})

it('the tint mix matches a known value', () => {
  expect(mix('#ffffff', '#000000', 100)).toBe('#ffffff')
  expect(mix('#ff0000', '#ff0000', 37)).toBe('#ff0000')
})

it('the desktop card light override matches the light theme', () => {
  const light = theme('light')
  const card = /:root\[data-theme='light'\] \{([\s\S]*?)\}/.exec(css)![1]
  for (const [k, v] of Object.entries(light)) expect(card).toContain(`--${k}: ${v}`)
})
