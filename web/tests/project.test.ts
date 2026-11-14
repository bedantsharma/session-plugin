import { render, screen } from '@testing-library/svelte'
import { describe, expect, it, vi } from 'vitest'

// LayerChart reads prefers-reduced-motion when it loads; jsdom has no matchMedia.
vi.hoisted(() => {
  window.matchMedia ??= ((q: string) => ({
    matches: false,
    media: q,
    addEventListener: () => {},
    removeEventListener: () => {},
  })) as unknown as typeof window.matchMedia
})
import Project from '../src/lib/drill/Project.svelte'
import type { Client } from '../src/lib/api'
import type { ProjectDrill } from '../src/lib/types'

const drill = (scope: ProjectDrill['scope']): ProjectDrill => ({
  repo_id: 'r1',
  name: 'shop',
  tree: [
    {
      id: 'G',
      shown: 'G',
      parent_id: null,
      kind: 'goal',
      title: 'Ship shop',
      intent: 'People can use it.',
      status: 'todo',
      rollup: 'in_progress',
      hill: null,
      stale: false,
      owner_checked: false,
      estimate_min: null,
      actual_min: 0,
      depends_on: [],
      external_deps: [],
      source_text: null,
      branch: null,
      decisions: 0,
      depth: 0,
      track: 1,
      check: 'tests',
      actions: [],
      remove: null,
    },
  ],
  run_number: 1,
  completed_runs: [],
  can_close_run: false,
  scope,
  replans: [],
  tracks: [],
  finished_tracks: [],
})

const client = (d: ProjectDrill): Client => ({
  get: async <T>() => d as T,
  act: async () => ({ ok: true, reply: '' }),
})

const props = (d: ProjectDrill) => ({
  client: client(d),
  repo: 'r1',
  version: 1,
  onOpenTask: () => {},
  onOpenHistory: () => {},
  onOpenTuning: () => {},
})

describe('Project drill', () => {
  it('shows the scope as numbers when every point shares one time', async () => {
    const ts = '2026-10-14T23:00:00Z'
    const { container } = render(
      Project,
      props(
        drill([
          { ts, total_min: 30, done_min: 0 },
          { ts, total_min: 75, done_min: 0 },
          { ts, total_min: 120, done_min: 10 },
        ]),
      ),
    )
    expect(await screen.findByTestId('scope-summary')).toHaveTextContent(
      'Scope 120 min · done 10 min',
    )
    expect(container.querySelector('.scope-chart')).toBeNull()
  })

  it('shows the numbers when the points span under a minute', async () => {
    const { container } = render(
      Project,
      props(
        drill([
          { ts: '2026-10-14T23:00:00Z', total_min: 30, done_min: 0 },
          { ts: '2026-10-14T23:00:40Z', total_min: 45, done_min: 5 },
        ]),
      ),
    )
    expect(await screen.findByTestId('scope-summary')).toHaveTextContent(
      'Scope 45 min · done 5 min',
    )
    expect(container.querySelector('.scope-chart')).toBeNull()
  })

  it('a parent shows its roll-up, not its stored to do', async () => {
    const d = drill([])
    d.tree[0] = { ...d.tree[0], rollup: 'done' }
    const { container } = render(Project, props(d))
    const goal = await vi.waitFor(() => {
      const el = container.querySelector('[data-node="G"]')
      if (!el) throw new Error('not yet')
      return el
    })
    expect(goal).toHaveTextContent('done')
    expect(goal).not.toHaveTextContent('to do')
  })

  it('the intent is behind a labelled control', async () => {
    render(Project, props(drill([])))
    const btn = await screen.findByRole('button', { name: /show intent/i })
    expect(btn).toHaveAttribute('aria-expanded', 'false')
  })

  it('a task shows where it came from, its branch and its links to other repos', async () => {
    const d = drill([])
    d.tree.push({
      ...d.tree[0],
      id: '1',
      parent_id: 'G',
      kind: 'task',
      title: 'Login page',
      intent: null,
      rollup: null,
      estimate_min: 30,
      source_text: 'from sp7-telemetry plan, task 3',
      branch: 'feat/auth',
      external_deps: [
        { label: 'api:2.4', title: 'Login endpoint', ready: true },
        { label: 'api:2.5', title: 'Logout', ready: false },
      ],
      depth: 1,
    })
    const { container } = render(Project, props(d))
    const task = await vi.waitFor(() => {
      const el = container.querySelector('[data-node="1"]')
      if (!el) throw new Error('not yet')
      return el
    })
    expect(task.querySelector('[data-testid="source"]')).toHaveTextContent(
      'from sp7-telemetry plan, task 3',
    )
    expect(task).toHaveTextContent('feat/auth')
    const deps = [...task.querySelectorAll('[data-testid="external-dep"]')].map(
      (e) => e.textContent,
    )
    expect(deps).toEqual(['after api:2.4 · ready', 'after api:2.5 · waiting'])
    // A node added by hand says nothing about where it came from.
    expect(container.querySelector('[data-node="G"] [data-testid="source"]')).toBeNull()
  })

  it('numbers restart per run, raw ids stay small, and finished runs stay readable', async () => {
    const d = drill([])
    d.run_number = 2
    d.can_close_run = true
    d.tree.push({
      ...d.tree[0],
      id: '3.1',
      shown: '1.1',
      parent_id: 'G',
      kind: 'task',
      title: 'Invoices',
      intent: null,
      rollup: null,
      depth: 1,
    })
    d.completed_runs = [
      {
        number: 1,
        title: 'Core',
        closed_at: '2026-10-20T10:00:00Z',
        tasks: 2,
        done_pct: 100,
        tree: [{ ...d.tree[0], id: '1.1', shown: '1.1', kind: 'task', title: 'Schema', depth: 1 }],
      },
    ]
    const { container } = render(Project, props(d))
    const task = await vi.waitFor(() => {
      const el = container.querySelector('[data-node="3.1"]')
      if (!el) throw new Error('not yet')
      return el
    })
    expect(task).toHaveTextContent('1.1')
    expect(task.querySelector('[data-testid="raw-id"]')).toHaveTextContent('(3.1)')
    expect(screen.getByTestId('run-number')).toHaveTextContent('Run 2')
    expect(screen.getByRole('button', { name: 'Close this run' })).toBeInTheDocument()
    const run = container.querySelector('[data-run="1"]')!
    expect(run).toHaveTextContent('Run 1: Core')
    expect(run).toHaveTextContent('2 tasks')
    expect(run).toHaveTextContent('100%')
  })

  it('offers no close button until the run is finished', async () => {
    render(Project, props(drill([])))
    await screen.findByRole('button', { name: /show intent/i })
    expect(screen.queryByRole('button', { name: 'Close this run' })).toBeNull()
  })

  it('shows one section per track, the finished ones with Reopen, and row actions', async () => {
    const base = drill([])
    const node = base.tree[0]
    const section = (no: number, title: string) => ({
      track: no,
      goal: { id: no === 1 ? 'G' : `T${no}-G`, shown: 'G', title },
      done_pct: no * 10,
      claimed_pct: 0,
      reviewed_tasks: no === 2 ? 2 : 0,
      unestimated: 0,
      forecast: null,
      run_number: 1,
      tree: [
        {
          ...node,
          id: no === 1 ? 'G' : `T${no}-G`,
          title,
          track: no,
          kind: 'goal',
        },
        {
          ...node,
          id: no === 1 ? '1.1' : `T${no}-1.1`,
          shown: '1.1',
          parent_id: no === 1 ? 'G' : `T${no}-G`,
          kind: 'task',
          title: `Task of ${title}`,
          status: 'claimed',
          track: no,
          depth: 1,
          actions: [
            {
              label: '✓ Works',
              action: {
                kind: 'verify',
                repo: 'r1',
                task: no === 1 ? '1.1' : `T${no}-1.1`,
                level: 4,
              },
              inputs: [],
              tone: 'primary' as const,
            },
          ],
          remove: {
            text: 'Remove task 1.1 for good?',
            action: { kind: 'remove', repo: 'r1', id: 'x', title: 'x', confirm: false },
          },
        },
      ],
      completed_runs: [],
      can_close_run: false,
      finish: null,
      remove: null,
    })
    const d: ProjectDrill = {
      ...base,
      tracks: [section(1, 'Ship shop'), section(2, 'Docs refresh')],
      finished_tracks: [
        {
          track: 3,
          goal: { id: 'T3-G', shown: 'G', title: 'Old spike' },
          done_pct: 100,
          reopen: {
            label: 'Reopen',
            action: { kind: 'reopen_track', repo: 'r1', track: 3 },
            inputs: [],
            tone: 'neutral',
          },
          remove: null,
        },
      ],
    }
    const { container } = render(Project, props(d))
    expect(await screen.findByRole('region', { name: 'Track 1: Ship shop' })).toBeInTheDocument()
    expect(screen.getByRole('region', { name: 'Track 2: Docs refresh' })).toBeInTheDocument()
    // The display id restarts from 1 in every track; the raw id stays in small text.
    expect(container.querySelector('[data-node="T2-1.1"]')).toHaveTextContent('1.1')
    expect(container.querySelector('[data-node="T2-1.1"]')).toHaveTextContent('(T2-1.1)')
    expect(screen.getAllByRole('button', { name: '✓ Works' })).toHaveLength(2)
    expect(container.querySelector('[data-track="2"]')).toHaveTextContent(
      'includes 2 tasks done without tests',
    )
    expect(container.querySelector('[data-finished="3"]')).toHaveTextContent('Old spike')
    expect(screen.getByRole('button', { name: 'Reopen' })).toBeInTheDocument()
  })
})
