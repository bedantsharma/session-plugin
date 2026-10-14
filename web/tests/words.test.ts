import { describe, expect, it } from 'vitest'
import { filesWord, hillWord, mins, sessionWord, unitWord } from '../src/lib/words'

describe('owner-facing words', () => {
  it('names the hill exploring/executing, never uphill/downhill, and hides it at 100%', () => {
    expect(hillWord('uphill', 40)).toMatchObject({ icon: '?', text: 'exploring', tone: 'warn' })
    expect(hillWord('downhill', 40)).toMatchObject({ icon: '→', text: 'executing', tone: 'ok' })
    expect(hillWord('uphill', 40)?.tip).toBe('Unknowns remain — this % may still change')
    expect(hillWord('downhill', 40)?.tip).toBe('Path is clear — this % is reliable')
    expect(hillWord('downhill', 100)).toBeNull()
    expect(hillWord(null, 10)).toBeNull()
  })

  it('labels units in words with their source', () => {
    const t = (l: string | null, s: string | null) => {
      const w = unitWord(l, s)
      return `${w.icon} ${w.text}`.trim()
    }
    expect(t('on_task', 'agent')).toBe('✓ On task · agent')
    expect(t('on_task', 'heuristic')).toBe('✓ On task · auto')
    expect(t('on_task', 'owner')).toBe('✓ On task · owner')
    expect(t('detour', 'heuristic')).toBe('↷ Detour · auto')
    expect(t('drift', 'owner')).toBe('▲ Drift · owner')
    expect(t(null, null)).toBe('Unlabelled')
    expect(unitWord('drift', 'agent').tone).toBe('bad')
  })

  it('gives each session state a word and a shape', () => {
    const w = (state_label: string, light: 'green' | 'amber' | 'red', light_word = '') =>
      sessionWord({ state_label, light, light_word })
    expect(w('working', 'green', 'on task')).toMatchObject({ icon: '●', text: 'Working' })
    expect(w('working', 'amber', 'detour')).toMatchObject({
      text: 'Working · detour',
      tone: 'warn',
    })
    expect(w('idle', 'amber')).toMatchObject({ icon: '◆', text: 'Idle', tone: 'warn' })
    expect(w('done', 'green')).toMatchObject({ icon: '✓', text: 'Done', tone: 'ok' })
    expect(w('needs you', 'red')).toMatchObject({ icon: '■', text: 'Needs you', tone: 'bad' })
  })

  it('reads durations and file counts as words', () => {
    expect(mins(20)).toBe('<1 min')
    expect(mins(360)).toBe('6 min')
    expect(filesWord(0)).toBe('no files')
    expect(filesWord(1)).toBe('1 file')
    expect(filesWord(4)).toBe('4 files')
  })
})
