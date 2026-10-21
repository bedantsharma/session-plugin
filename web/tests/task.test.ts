import { render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import Task from '../src/lib/drill/Task.svelte'
import type { Client } from '../src/lib/api'
import type { TaskDrill } from '../src/lib/types'
import { COST } from './fixtures'

const drill: TaskDrill = {
  repo_id: 'r1',
  id: '1.1',
  title: 'Login form',
  status: 'l2',
  estimate_min: 30,
  actual_min: 54,
  decisions: [],
  verifications: [],
  actors: [{ agent_type: null, model: 'claude-opus-5-5' }],
  retries: 2,
  usage: {
    total: COST,
    by_model: [
      { label: 'Opus 5.5', tokens: 3_000_000, tokens_text: '3M', cost_text: '$12.40' },
      { label: 'mystery', tokens: 100_000, tokens_text: '100k', cost_text: 'cost unknown' },
    ],
    by_agent: [
      { label: 'Main agent', tokens: 3_100_000, tokens_text: '3.1M', cost_text: '$12.40' },
    ],
  },
  steps: [
    { id: '1.1.1', title: 'Form', status: 'l2', current: false },
    { id: '1.1.2', title: 'Errors', status: 'active', current: true },
  ],
  ratio: 1.8,
  withdraw: null,
}

const client: Client = {
  get: async <T>() => drill as T,
  act: async () => ({ ok: true, reply: '' }),
}

describe('Task drill', () => {
  it('shows estimate vs actual with the ratio, retries, steps and cost', async () => {
    const { container } = render(Task, { client, repo: 'r1', id: '1.1', version: 1 })
    expect(await screen.findByTestId('task-time')).toHaveTextContent(
      '54 of 30 min · took 1.8× its estimate',
    )
    expect(screen.getByTestId('task-cost')).toHaveTextContent('$12.40 spent · 3.1M tokens')
    expect(container.querySelector('[data-step="1.1.2"]')).toHaveTextContent(
      'Errors · being worked on · current',
    )
    expect(container.querySelector('[data-step="1.1.1"]')).not.toHaveTextContent('current')
    const rows = [...screen.getByRole('table', { name: 'By model' }).querySelectorAll('tbody tr')]
    const cells = rows.map((r) => [...r.querySelectorAll('td')].map((c) => c.textContent?.trim()))
    expect(cells).toEqual([
      ['Opus 5.5', '3M', '$12.40'],
      ['mystery', '100k', 'cost unknown'],
    ])
    expect(screen.getByRole('table', { name: 'By agent' })).toHaveTextContent('Main agent')
    expect(container.textContent).not.toMatch(/[a-z]_[a-z]/)
  })

  it('offers "Take back sign-off" and sends the withdraw through the owner action', async () => {
    const sent: unknown[] = []
    const d: TaskDrill = {
      ...drill,
      status: 'l4',
      withdraw: {
        label: 'Take back sign-off',
        action: { kind: 'withdraw', repo: 'r1', task: '1.1' },
        inputs: [],
        tone: 'neutral',
      },
    }
    const c: Client = {
      get: async <T>() => d as T,
      act: async (a) => {
        sent.push(a)
        return { ok: false, status: 409, error: 'nothing to withdraw on 1.1' }
      },
    }
    render(Task, { client: c, repo: 'r1', id: '1.1', version: 1 })
    const btn = await screen.findByRole('button', { name: 'Take back sign-off' })
    btn.click()
    expect(await screen.findByRole('status')).toHaveTextContent('nothing to withdraw on 1.1')
    expect(sent).toEqual([{ kind: 'withdraw', repo: 'r1', task: '1.1' }])
  })
})
