import { render, screen } from '@testing-library/svelte'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import Header from '../src/lib/glance/Header.svelte'
import { Poller, type ConnState } from '../src/lib/api'
import { glance, item } from './fixtures'

const up: ConnState = { connected: true, unauthorized: false, lastOk: 0 }

describe('Header', () => {
  it('shows the count in a red pill, or All clear', () => {
    const { rerender } = render(Header, { glance: glance([item()]), conn: up })
    expect(screen.getByTestId('pill')).toHaveTextContent('1 need you')
    expect(screen.queryByTestId('all-clear')).toBeNull()
    rerender({ glance: glance([]), conn: up })
    expect(screen.getByTestId('all-clear')).toHaveTextContent('All clear')
  })

  it('escalates past 4: emphasised, says show all, pulses a bounded number of times', () => {
    const five = [1, 2, 3, 4, 5].map((i) => item({ id: `x${i}` }))
    render(Header, { glance: glance(five), conn: up })
    const pill = screen.getByTestId('pill')
    expect(pill).toHaveTextContent('5 need you · show all')
    expect(pill.className).toContain('pulse-3')
    // Filled red with dark text; never red text on the red fill.
    expect(pill.className).toContain('text-on-accent')
    expect(pill.className).not.toContain('text-bad')
  })

  it('says Disconnected with the last update time, never All clear', () => {
    const lastOk = new Date(2026, 9, 14, 9, 5).getTime()
    render(Header, { glance: glance([]), conn: { connected: false, unauthorized: false, lastOk } })
    expect(screen.getByTestId('disconnected')).toHaveTextContent(
      'Disconnected — last updated 09:05',
    )
    expect(screen.queryByText(/All clear/)).toBeNull()
  })
})

describe('disconnected_never_shows_all_clear', () => {
  beforeEach(() => vi.useFakeTimers())
  afterEach(() => vi.useRealTimers())

  it('two failed polls replace All clear with Disconnected', async () => {
    let conn: ConnState = up
    const g = glance([])
    const { rerender } = render(Header, { glance: g, conn })
    expect(screen.getByTestId('all-clear')).toBeInTheDocument()
    let calls = 0
    const p = new Poller({
      version: async () => {
        calls += 1
        if (calls > 1) throw new Error('server gone')
        return 1
      },
      onChange: () => {},
      onState: (s) => (conn = s),
      hidden: () => false,
    })
    p.start()
    await vi.advanceTimersByTimeAsync(0)
    await vi.advanceTimersByTimeAsync(500)
    await vi.advanceTimersByTimeAsync(1000)
    p.stop()
    expect(conn.connected).toBe(false)
    await rerender({ glance: g, conn })
    expect(screen.queryByText(/All clear/)).toBeNull()
    expect(screen.getByTestId('disconnected')).toBeInTheDocument()
  })
})
