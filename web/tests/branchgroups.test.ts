import { render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import BranchGroups from '../src/lib/glance/BranchGroups.svelte'
import type { BranchGroup } from '../src/lib/types'

const group = (over: Partial<BranchGroup> = {}): BranchGroup => ({
  branch: 'feat/auth',
  repos: [
    { repo_id: 'a', name: 'api', done_pct: 20, claimed_pct: 10, sessions: 1 },
    { repo_id: 'w', name: 'web', done_pct: null, claimed_pct: null, sessions: 2 },
  ],
  done_pct: 45,
  claimed_pct: 5,
  ...over,
})

describe('Branch groups', () => {
  it('renders one line per group with the combined bar and each repo in the tooltip', () => {
    const { container } = render(BranchGroups, { groups: [group()] })
    const li = container.querySelector('[data-branch="feat/auth"]')!
    expect(li).toHaveTextContent('feat/auth')
    expect(li).toHaveTextContent('45% · 2 repos')
    expect(li).toHaveAttribute('title', 'api 20% · web no tasks on this branch yet')
    expect(screen.getByRole('img')).toHaveAccessibleName('45% done, 5% claimed')
  })

  it('says so when no task is on the branch yet', () => {
    const { container } = render(BranchGroups, {
      groups: [group({ done_pct: null, claimed_pct: null })],
    })
    expect(container).toHaveTextContent('no tasks on this branch yet · 2 repos')
  })

  it('is hidden without groups', () => {
    const { container } = render(BranchGroups, { groups: [] })
    expect(container.querySelector('[data-testid="branch-groups"]')).toBeNull()
  })
})
