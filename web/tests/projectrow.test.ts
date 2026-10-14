import { render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import ProjectRow from '../src/lib/glance/ProjectRow.svelte'
import { project } from './fixtures'

describe('ProjectRow', () => {
  it('draws done and claimed apart and says both in text', () => {
    render(ProjectRow, { project: project() })
    expect(screen.getByTestId('done-bar').style.width).toBe('62%')
    expect(screen.getByTestId('claimed-bar').style.width).toBe('14%')
    expect(screen.getByTestId('claimed-bar').className).toContain('hatch')
    expect(screen.getByTestId('progress-text').textContent?.replace(/\s+/g, ' ')).toContain(
      '62% done · +14% claimed',
    )
    expect(screen.getByRole('img')).toHaveAccessibleName('62% done, 14% claimed')
  })

  it('shows uphill as a text pill with its % muted, and stuck as a badge', () => {
    const { container } = render(ProjectRow, { project: project() })
    const uphill = container.querySelector('[data-milestone="1"]')!
    expect(uphill).toHaveTextContent('uphill')
    expect(screen.getByTestId('uphill-pct').className).toContain('text-muted')
    expect(uphill).toHaveTextContent('stuck · 3 sessions')
    expect(container.querySelector('[data-milestone="2"]')).toHaveTextContent('downhill')
  })
})
