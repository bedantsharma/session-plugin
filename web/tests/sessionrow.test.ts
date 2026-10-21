import { render } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import SessionRow from '../src/lib/glance/SessionRow.svelte'
import type { SessionRow as Row } from '../src/lib/types'

const row = (over: Partial<Row> = {}): Row => ({
  session_id: 's1aaaa-full',
  short: 's1aaaa',
  repo_id: 'r1',
  state_label: 'idle',
  light: 'amber',
  light_word: 'waiting',
  task: { id: '1.1', title: 'Login page' },
  detour: null,
  units: 2,
  active_min: 12,
  nav_text: 'on task',
  flags: [],
  rest: { kind: 'idle' },
  asked_question: false,
  branch: 'feat/auth',
  worktree: null,
  cost: null,
  waiting: [{ wait_id: 'W1', target: '2.4 Login form', since_min: 40 }],
  rank: 4,
  ...over,
})

describe('Session row', () => {
  it('a waiting session gets the amber Waiting chip and says what it waits on', () => {
    const { container } = render(SessionRow, { session: row() })
    const chip = container.querySelector('[data-chip]')!
    expect(chip).toHaveTextContent('Waiting')
    expect(container.querySelector('[data-testid="waiting"]')).toHaveTextContent(
      '· waiting on 2.4 Login form',
    )
  })

  it('without waits nothing is added', () => {
    const { container } = render(SessionRow, {
      session: row({ light_word: 'idle', waiting: [] }),
    })
    expect(container.querySelector('[data-chip]')).toHaveTextContent('Idle')
    expect(container.querySelector('[data-testid="waiting"]')).toBeNull()
  })
})
