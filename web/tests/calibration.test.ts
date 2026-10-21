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
  // Charts measure their box; jsdom has no ResizeObserver.
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver
})
import Calibration from '../src/lib/drill/Calibration.svelte'
import type { Client } from '../src/lib/api'
import type { CalibrationDrill, CalibrationPoint } from '../src/lib/types'
import { forecast } from './fixtures'

const point = (id: string, ratio: number, at: string): CalibrationPoint => ({
  task_id: id,
  title: `Task ${id}`,
  estimate_min: 30,
  actual_min: 30 * ratio,
  ratio,
  model: 'claude-opus-5-5',
  completed_at: at,
})

const PRICE = { input: 4, output: 20, cache_read: 0.2, cache_write_5m: 5, cache_write_1h: 8 }

function drill(over: Partial<CalibrationDrill> = {}): CalibrationDrill {
  return {
    repo_id: 'r1',
    name: 'shop',
    min_points: 3,
    model_points: 6,
    models: [],
    pooled: [],
    pooled_text: null,
    forecast: forecast('Not enough finished tasks to forecast yet — 3 more are needed'),
    history: [],
    prices: [{ model: 'claude-opus-5-5', name: 'Opus 5.5', price: PRICE, source: 'default' }],
    unpriced_models: [],
    notes: [],
    ...over,
  }
}

const client = (d: CalibrationDrill): Client => ({
  get: async <T>() => d as T,
  act: async () => ({ ok: true, reply: '' }),
})

describe('Calibration drill', () => {
  it('says what is missing when nothing is finished yet', async () => {
    render(Calibration, { client: client(drill()), repo: 'r1', version: 1 })
    expect(await screen.findByTestId('calibration-forecast')).toHaveTextContent(
      'Not enough finished tasks to forecast yet — 3 more are needed',
    )
    expect(screen.getByTestId('calibration-forecast').textContent).not.toMatch(/likely|Oct/)
    expect(screen.getByTestId('no-points')).toHaveTextContent('The forecast needs 3.')
    expect(screen.getByTestId('no-history')).toBeInTheDocument()
    expect(screen.getByText('Opus 5.5')).toBeInTheDocument()
  })

  it('lists the ratios as numbers when one point cannot make a chart, and flags unpriced models', async () => {
    const p = point('1.1', 2, '2026-10-14T09:00:00Z')
    const d = drill({
      models: [
        {
          model: 'claude-opus-5-5',
          name: 'Opus 5.5',
          points: [p],
          median_ratio: 2,
          text: 'tasks typically take 2× their estimate',
          own: false,
        },
      ],
      pooled: [p],
      pooled_text: 'tasks typically take 2× their estimate',
      forecast: forecast('Not enough finished tasks to forecast yet — 2 more are needed'),
      history: [
        {
          ts: '2026-10-14T09:01:00Z',
          ...forecast('Not enough finished tasks to forecast yet — 2 more are needed'),
        },
      ],
      unpriced_models: ['mystery'],
      notes: ['No price for mystery: add one under [prices] in config.toml.'],
    })
    const { container } = render(Calibration, { client: client(d), repo: 'r1', version: 1 })
    expect(await screen.findByTestId('ratio-list')).toHaveTextContent(
      '1.1 Task 1.1 · 60 of 30 min (2×)',
    )
    expect(container.querySelector('[data-model="claude-opus-5-5"]')).toHaveTextContent(
      'tasks typically take 2× their estimate',
    )
    expect(screen.getByTestId('history-list')).toHaveTextContent('Not enough finished tasks')
    expect(screen.getByRole('note')).toHaveTextContent('No price for mystery')
    expect(container.querySelector('[data-unpriced="mystery"]')).toHaveTextContent(
      'no price — cost unknown',
    )
  })

  it('draws the dots and the band once there are points at different times', async () => {
    const pts = [
      point('1.1', 1, '2026-10-14T09:00:00Z'),
      point('1.2', 2, '2026-10-14T12:00:00Z'),
      point('1.3', 1.5, '2026-10-15T09:00:00Z'),
    ]
    const d = drill({
      models: [
        {
          model: 'claude-opus-5-5',
          name: 'Opus 5.5',
          points: pts,
          median_ratio: 1.5,
          text: 'tasks typically take 1.5× their estimate',
          own: false,
        },
      ],
      pooled: pts,
      pooled_text: 'tasks typically take 1.5× their estimate',
      forecast: forecast(),
      history: [
        { ts: '2026-10-15T09:00:00Z', ...forecast() },
        { ts: '2026-10-16T09:00:00Z', ...forecast(), p50_hours: 8 },
      ],
    })
    const { container } = render(Calibration, { client: client(d), repo: 'r1', version: 1 })
    expect(await screen.findByTestId('calibration-forecast')).toHaveTextContent(
      'likely by Oct 30 · could take until Nov 8',
    )
    expect(screen.queryByTestId('ratio-list')).toBeNull()
    expect(screen.queryByTestId('history-list')).toBeNull()
    expect(
      container.querySelector('[aria-label="Agent hours left, 50% and 90% chance, over time"]'),
    ).not.toBeNull()
  })
})
