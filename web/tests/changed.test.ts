import { render } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import { changed } from '../src/lib/changed'
import NeedsYou from '../src/lib/glance/NeedsYou.svelte'
import { item, question } from './fixtures'

describe('change cues', () => {
  it('skip the first value, then mark each change once', () => {
    const node = document.createElement('div')
    const a = changed(node, { value: 1 })
    expect(node.classList.contains('flash')).toBe(false)
    a.update({ value: 1 })
    expect(node.classList.contains('flash')).toBe(false)
    a.update({ value: 2 })
    expect(node.classList.contains('flash')).toBe(true)
    node.dispatchEvent(Object.assign(new Event('animationend'), {}))
    expect(node.classList.contains('flash')).toBe(false)
    a.update({ value: 3, cls: 'num' })
    expect(node.classList.contains('num')).toBe(true)
    a.destroy()
  })

  it('needs-you items there at first load just appear; later ones arrive', async () => {
    const { rerender, container } = render(NeedsYou, { items: [item()], act: vi.fn() })
    const first = container.querySelector('[data-item]')!
    expect(first.className).not.toContain('arrive')
    await rerender({ items: [item(), question('later')], act: vi.fn() })
    const later = container.querySelector('[data-item="question:later"]')!
    expect(later.className).toContain('arrive')
  })

  it('takes the class off even when no animation ran (Motion off)', () => {
    vi.useFakeTimers()
    try {
      const node = document.createElement('div')
      const a = changed(node, { value: 1 })
      a.update({ value: 2 })
      expect(node.classList.contains('flash')).toBe(true)
      vi.advanceTimersByTime(2000)
      expect(node.classList.contains('flash')).toBe(false)
      a.destroy()
    } finally {
      vi.useRealTimers()
    }
  })
})
