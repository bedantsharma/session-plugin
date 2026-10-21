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
      hill: null,
      stale: false,
      owner_checked: false,
      estimate_min: null,
      actual_min: 0,
      depends_on: [],
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

  it('the intent is behind a labelled control', async () => {
    render(Project, props(drill([])))
    const btn = await screen.findByRole('button', { name: /show intent/i })
    expect(btn).toHaveAttribute('aria-expanded', 'false')
  })
})
