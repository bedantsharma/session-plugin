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
  task: { id: '1.1', title: 'Login form' },
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
    },
  ],
  split: { on_task_s: 360, drift_s: 0, unlabeled_s: 30, detours: [] },
  suspended: [],
  checkpoints: [],
  leftovers: [],
  handoff_note: null,
  readback: 'not needed',
  owner_answers: [],
}

const client: Client = {
  get: async <T>() => drill as T,
  act: async () => ({ ok: true, reply: '' }),
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
})
