import { render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import ProjectRow from '../src/lib/glance/ProjectRow.svelte'
import { COST, forecast, project } from './fixtures'

describe('ProjectRow', () => {
  it('draws done and claimed apart and says both in text', () => {
    render(ProjectRow, { project: project() })
    expect(screen.getByTestId('done-bar').style.width).toBe('62%')
    expect(screen.getByTestId('claimed-bar').style.width).toBe('14%')
    expect(screen.getByTestId('claimed-bar').className).toContain('hatch')
    const big = screen.getByTestId('progress-text').textContent?.replace(/\s+/g, ' ')
    expect(big).toContain('62%')
    expect(big).toContain('+14% claimed')
    expect(screen.getByTestId('progress-caption').textContent?.replace(/\s+/g, ' ')).toContain(
      'Done 62% · 0 of 2 milestones complete',
    )
    // The project bar is the one labelled image; milestone mini bars are hidden.
    expect(screen.getByRole('img')).toHaveAccessibleName('62% done, 14% claimed')
  })

  it('names the hill exploring/executing with a tooltip, mutes an exploring %, shows stuck', () => {
    const { container } = render(ProjectRow, { project: project() })
    const exploring = container.querySelector('[data-milestone="1"]')!
    expect(exploring).toHaveTextContent('exploring')
    expect(exploring).not.toHaveTextContent('uphill')
    expect(exploring.querySelector('[title]')).toHaveAttribute(
      'title',
      'Unknowns remain — this % may still change',
    )
    expect(screen.getByTestId('exploring-pct').className).toContain('text-muted')
    expect(exploring).toHaveTextContent('stuck · 3 sessions')
    const executing = container.querySelector('[data-milestone="2"]')!
    expect(executing).toHaveTextContent('executing')
    expect(executing).not.toHaveTextContent('downhill')
  })

  it('hides the hill chip on a finished milestone', () => {
    const p = project()
    p.milestones[1] = { ...p.milestones[1], done_pct: 100 }
    const { container } = render(ProjectRow, { project: p })
    expect(container.querySelector('[data-milestone="2"]')).not.toHaveTextContent('executing')
    expect(screen.getByTestId('progress-caption')).toHaveTextContent('1 of 2 milestones complete')
  })

  it('shows 6 milestones, then all on request', async () => {
    const p = project()
    p.milestones = Array.from({ length: 8 }, (_, i) => ({
      ...p.milestones[1],
      id: String(i + 1),
      title: `M${i + 1}`,
    }))
    const { container } = render(ProjectRow, { project: p })
    expect(container.querySelectorAll('[data-milestone]')).toHaveLength(6)
    screen.getByRole('button', { name: 'show all 8' }).click()
    await Promise.resolve()
    await new Promise((r) => setTimeout(r))
    expect(container.querySelectorAll('[data-milestone]')).toHaveLength(8)
  })

  it('shows the forecast range with what it means, and the cost', () => {
    render(ProjectRow, { project: project({ forecast: forecast(), cost: COST }) })
    const line = screen.getByTestId('forecast')
    expect(line.textContent?.replace(/\s+/g, ' ').trim()).toBe(
      'likely by Oct 30 · could take until Nov 8 (50% and 90% chance, based on how long 5 verified tasks actually took)',
    )
    expect(line.querySelector('[title]')).toHaveAttribute(
      'title',
      '50% and 90% chance, based on how long 5 verified tasks actually took',
    )
    expect(screen.getByTestId('cost')).toHaveTextContent('$12.40 spent · 3.1M tokens')
  })

  it('shows the reason and no date when the data is too thin', () => {
    const reason = 'Not enough finished tasks to forecast yet — 2 more are needed'
    render(ProjectRow, { project: project({ forecast: forecast(reason) }) })
    const line = screen.getByTestId('forecast')
    expect(line).toHaveTextContent(reason)
    expect(line.textContent).not.toMatch(/likely|Oct|Nov/)
    expect(screen.queryByTestId('cost')).toBeNull()
  })
})
