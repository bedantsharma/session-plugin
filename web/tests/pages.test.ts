import { render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import Pages from '../src/lib/drill/Pages.svelte'
import type { Client } from '../src/lib/api'
import type { PagesView } from '../src/lib/types'

const TITLES = [
  'What this project is for',
  'How it fits together',
  'Where things stand',
  'Why we chose what we chose',
  'Words we use',
]

const setUp = (): PagesView => ({
  state: 'set_up',
  pages: TITLES.map((title, i) => ({
    title,
    updated: `2026-10-2${i}`,
    text: i === 2 ? '# Current state\n<b>not bold</b> all green' : `Text of ${title}`,
  })),
})

const client = (v: PagesView): Client => ({
  get: async <T>() => v as T,
  act: async () => ({ ok: true, reply: '' }),
})

describe('Project pages view', () => {
  it('pages_view_renders_titles_and_dates', async () => {
    const { container } = render(Pages, { client: client(setUp()), repo: 'r1', version: 1 })
    for (const t of TITLES) await screen.findByText(t)
    for (let i = 0; i < 5; i++) expect(container.textContent).toContain(`2026-10-2${i}`)
    // Read-only: no buttons, no inputs.
    expect(container.querySelectorAll('button, input, textarea')).toHaveLength(0)
  })

  it('shows page text as text, never as HTML', async () => {
    const { container } = render(Pages, { client: client(setUp()), repo: 'r1', version: 1 })
    await screen.findByText('Where things stand')
    expect(container.querySelector('b')).toBeNull()
    expect(container.textContent).toContain('<b>not bold</b> all green')
  })

  it('pages_view_not_set_up_shows_command', async () => {
    const { container } = render(Pages, {
      client: client({ state: 'not_set_up', pages: [] }),
      repo: 'r1',
      version: 1,
    })
    await screen.findByText(/aren’t set up for this project yet/)
    expect(container.textContent).toContain('bridge-screen pages set-up')
    for (const t of TITLES) expect(container.textContent).not.toContain(t)
  })

  it('says plainly when the owner chose not to have them', async () => {
    const { container } = render(Pages, {
      client: client({ state: 'declined', pages: [] }),
      repo: 'r1',
      version: 1,
    })
    await screen.findByText(/You chose not to have project pages/)
    expect(container.textContent).toContain('bridge-screen pages set-up')
  })
})
