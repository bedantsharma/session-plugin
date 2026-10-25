import { render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import Session from '../src/lib/drill/Session.svelte'
import type { Client } from '../src/lib/api'
import type { SessionDrill } from '../src/lib/types'

const drill: SessionDrill = {
  session_id: 's1aaaa-full',
  short: 's1aaaa',
  repo_id: 'r1',
  state: 'working',
  branch: 'feat/x',
  worktree: '/wt/x',
  task: { id: '1.1', shown: '1.1', title: 'Login form' },
  units: [
    {
      unit_no: 1,
      started_at: '2026-10-14T09:12:00Z',
      stopped_at: '2026-10-14T09:18:00Z',
      active_s: 360,
      task_id: '1.1',
      label: 'on_task',
      label_source: 'heuristic',
      drift_score: 0.4,
      detour: null,
      files: 1,
      disagreement: false,
      tokens: 12_400,
      tokens_text: '12k',
    },
    {
      unit_no: 2,
      started_at: '2026-10-14T09:20:00Z',
      stopped_at: '2026-10-14T09:20:30Z',
      active_s: 30,
      task_id: '1.1',
      label: null,
      label_source: null,
      drift_score: null,
      detour: null,
      files: 0,
      disagreement: false,
      tokens: 0,
      tokens_text: null,
    },
  ],
  split: { on_task_s: 360, requested_s: 0, drift_s: 0, unlabeled_s: 30, detours: [] },
  suspended: [],
  checkpoints: [],
  leftovers: [
    {
      leftover_id: 'L1',
      path: 'src/a.ts',
      task_id: null,
      suggested: 'resume',
      state: 'report_only',
      resolution: null,
    },
  ],
  handoff_note: null,
  // The API's codes, as the server sends them.
  readback: 'not_needed',
  owner_answers: [{ ts: '2026-10-14T09:30:00Z', answer: 'make_task', reason: null }],
  cost: {
    tokens: 12_400,
    tokens_text: '12k',
    usd: 0.09,
    unpriced_models: [],
    text: '$0.09',
    summary: '$0.09 spent · 12k tokens',
  },
  waits: [
    {
      wait_id: 'W1',
      target: 'api:2.4 Login endpoint',
      reason: 'the page calls it',
      open: true,
      state_text: 'waiting 40m',
      actions: [
        {
          label: 'Clear',
          action: { kind: 'clear_wait', wait_id: 'W1' },
          inputs: [],
          tone: 'neutral',
        },
        {
          label: 'Make it a plan link',
          action: { kind: 'promote_wait', wait_id: 'W1' },
          inputs: [],
          tone: 'primary',
        },
      ],
    },
    {
      wait_id: 'W2',
      target: "Sam's API key",
      reason: 'staging',
      open: false,
      state_text: 'cleared by hand',
      actions: [],
    },
  ],
}

const acted: unknown[] = []
const client: Client = {
  get: async <T>() => drill as T,
  act: async (a) => {
    acted.push(a)
    return { ok: true, reply: 'Cleared W1.' }
  },
}

describe('Session drill', () => {
  it('shows human unit labels, durations and file counts in a fixed-column table', async () => {
    const { container } = render(Session, { client, id: 's1aaaa', version: 1 })
    expect(await screen.findByText('Time by label')).toBeInTheDocument()
    const u1 = container.querySelector('[data-unit="1"]')!
    expect(u1).toHaveTextContent('On task · auto')
    expect(u1).toHaveTextContent('6 min')
    expect(u1).toHaveTextContent('1 file · score 0.40')
    expect(u1).not.toHaveTextContent('on_task')
    const u2 = container.querySelector('[data-unit="2"]')!
    expect(u2).toHaveTextContent('Unlabelled')
    expect(u2).toHaveTextContent('<1 min')
    expect(u2).toHaveTextContent('no files')
    // Labels never wrap.
    expect(u1.querySelector('[data-chip]')!.className).toContain('whitespace-nowrap')
    expect(screen.getByRole('img')).toHaveAccessibleName(/On task 6 min/)
  })

  it('never shows the snake_case codes the API sends', async () => {
    const { container } = render(Session, { client, id: 's1aaaa', version: 1 })
    expect(await screen.findByTestId('readback')).toHaveTextContent(
      'Handoff understood: not needed',
    )
    expect(screen.getByText('make it a task')).toBeInTheDocument()
    expect(
      screen.getByText('for information only — suggested: resume (carry on with it)'),
    ).toBeInTheDocument()
    expect(container.textContent).not.toMatch(/[a-z]_[a-z]/)
  })

  it('shows the session cost and the tokens of each unit', async () => {
    const { container } = render(Session, { client, id: 's1aaaa', version: 1 })
    expect(await screen.findByTestId('session-cost')).toHaveTextContent('$0.09 spent · 12k tokens')
    const tokens = [...container.querySelectorAll('[data-testid="unit-tokens"]')].map(
      (e) => e.textContent,
    )
    expect(tokens).toEqual(['12k', '—'])
  })

  it('lists open and cleared waits, with the owner buttons on open ones', async () => {
    render(Session, { client, id: 's1aaaa', version: 1 })
    const waits = await screen.findByTestId('waits')
    expect(waits).toHaveTextContent('api:2.4 Login endpoint — the page calls it')
    expect(waits).toHaveTextContent('waiting 40m')
    expect(waits).toHaveTextContent("Sam's API key — staging")
    expect(waits).toHaveTextContent('cleared by hand')
    const buttons = [...waits.querySelectorAll('button')].map((b) => b.textContent?.trim())
    expect(buttons).toEqual(['Clear', 'Make it a plan link'])
    ;(waits.querySelector('button') as HTMLButtonElement).click()
    expect(await screen.findByRole('status')).toHaveTextContent('Cleared W1.')
    expect(acted).toEqual([{ kind: 'clear_wait', wait_id: 'W1' }])
  })
})
