import { describe, expect, it } from 'vitest'
import { addedGroups, addedText } from '../src/lib/scope'

describe('scope summary', () => {
  it('reads several additions in the same minute as one entry', () => {
    const at = (m: number, s: number) =>
      `2026-10-21T10:${String(m).padStart(2, '0')}:${String(s).padStart(2, '0')}Z`
    const scope = [
      { ts: at(0, 0), total_min: 60 },
      { ts: at(2, 1), total_min: 80 },
      { ts: at(2, 20), total_min: 95 },
      { ts: at(2, 59), total_min: 105 },
      { ts: at(3, 0), total_min: 100 },
      { ts: at(5, 0), total_min: 130 },
    ]
    const groups = addedGroups(scope)
    expect(groups.map((g) => [g.count, g.by])).toEqual([
      [3, 45],
      [1, 30],
    ])
    expect(addedText(groups)).toMatch(
      /^3 additions, \+45 min at \d\d:\d\d · \+30 min at \d\d:\d\d$/,
    )
    expect(addedGroups([])).toEqual([])
  })
})
