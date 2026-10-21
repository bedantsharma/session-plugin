// No raw codes on any screen (plan Task 7): the shared list in src/lib/jargon.json,
// checked against what the dashboard, the card and the drills render.
import { render, screen } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'
import NeedsYou from '../src/lib/glance/NeedsYou.svelte'
import ProjectRow from '../src/lib/glance/ProjectRow.svelte'
import Task from '../src/lib/drill/Task.svelte'
import Card from '../src/card/Card.svelte'
import type { CardHost, ToolResult } from '../src/card/host'
import type { Client } from '../src/lib/api'
import type { TaskDrill } from '../src/lib/types'
import { FLAG, STATUS, jargonIn, levelWord, readbackWord, statusWord } from '../src/lib/words'
import { flagText } from '../src/lib/format'
import { COST, forecast, glance, item, leftover, project, question } from './fixtures'

const clean = (text: string | null | undefined) => expect(jargonIn(text ?? '')).toEqual([])

describe('no jargon on screens', () => {
  it('finds the codes it is meant to find', () => {
    expect(jargonIn('checkpoint missed · L2 · drift_red · read-back pending')).toEqual([
      'checkpoint',
      'L2',
      'drift_red',
      'read-back',
    ])
    expect(jargonIn('tests pass · you signed it off')).toEqual([])
  })

  it('every status, level, flag and read-back word reads in words', () => {
    for (const code of Object.keys(STATUS)) clean(statusWord(code).word)
    for (const level of [0, 1, 2, 3, 4]) clean(levelWord(level))
    for (const kind of Object.keys(FLAG)) clean(flagText({ kind, age_s: 60, tasks: ['1.2'] }))
    for (const s of ['not_needed', 'pending', 'ok', 'mismatch']) clean(readbackWord(s))
  })

  it('the needs-you queue and a project card', () => {
    const q = render(NeedsYou, { items: [item(), leftover(), question()], act: vi.fn() })
    clean(q.container.textContent)
    q.unmount()
    const p = render(ProjectRow, {
      project: project({ forecast: forecast(), cost: COST, goal_status: 'done', unestimated: 2 }),
    })
    clean(p.container.textContent)
  })

  it('the task drill with its verifications', async () => {
    const d: TaskDrill = {
      repo_id: 'r1',
      id: '1.1',
      title: 'Login form',
      status: 'l4',
      estimate_min: 30,
      actual_min: 20,
      decisions: [],
      verifications: [
        {
          verif_id: 'v1',
          level: 2,
          by: 'rerun',
          rerun: 'timed_out',
          sha: 'abcdef12',
          evidence: 'ok',
          state: 'stale',
          ts: '2026-10-21T10:00:00Z',
        },
        {
          verif_id: 'v2',
          level: 4,
          by: 'owner_via_agent',
          rerun: null,
          sha: 'abcdef12',
          evidence: 'looks right',
          state: 'withdrawn',
          ts: '2026-10-21T10:00:00Z',
        },
      ],
      actors: [],
      retries: 0,
      usage: null,
      steps: [],
      ratio: null,
      withdraw: null,
    }
    const client: Client = {
      get: async <T>() => d as T,
      act: async () => ({ ok: true, reply: '' }),
    }
    const { container } = render(Task, { client, repo: 'r1', id: '1.1', version: 1 })
    await screen.findByText(/tests pass/)
    clean(container.textContent)
  })

  it('the desktop card', async () => {
    const host: CardHost = {
      start: async () => ({}),
      onToolResult(cb: (r: ToolResult) => void) {
        setTimeout(() => cb({ structuredContent: glance([item(), leftover(), question()]) }))
      },
      onHostContext() {},
      callTool: async () => ({}),
      sizeChanged() {},
    }
    const { container } = render(Card, { host })
    await screen.findByText(/far off its task/)
    clean(container.textContent)
  })
})
