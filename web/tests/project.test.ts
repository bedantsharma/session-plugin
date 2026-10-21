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
    },
  ],
  scope,
  replans: [],
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
})
