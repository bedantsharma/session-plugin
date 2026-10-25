import { fireEvent, render, screen } from '@testing-library/svelte'
import { describe, expect, it } from 'vitest'
import History from '../src/lib/drill/History.svelte'
import { kindOptions, kindWords, sentence, taskText } from '../src/lib/drill/historyText'
import type { Client } from '../src/lib/api'
import type { HistoryPage } from '../src/lib/types'
import jargon from '../src/lib/jargon.json'

const page: HistoryPage = {
  events: [
    {
      pos: 3,
      ts: '2026-10-26T10:05:00Z',
      kind: 'verified',
      session_id: 'a1b2c3dddd',
      task_id: '4',
      task_shown: '4',
      task_title: 'Login form',
      summary: 'Task 4 (Login form) passed its tests',
      data: { level: 2, verif_id: 'v1' },
    },
    {
      pos: 2,
      ts: '2026-10-26T10:00:00Z',
      kind: 'task_claimed',
      session_id: 'a1b2c3dddd',
      task_id: '4',
      task_shown: '4',
      task_title: 'Login form',
      summary: 'Task 4 (Login form) claimed by session a1b2c3',
      data: { mode: 'auto' },
    },
    {
      pos: 1,
      ts: '2026-10-26T09:00:00Z',
      kind: 'brand_new_thing',
      session_id: 'owner',
      task_id: null,
      data: { x: 1 },
    },
  ],
  kinds: [
    { kind: 'verified', label: 'Checked and passed' },
    { kind: 'task_claimed', label: 'Task claimed' },
    { kind: 'brand_new_thing', label: 'Brand new thing' },
  ],
  next_before: null,
}

const client = (): Client => ({
  get: async <T>() => page as T,
  act: async () => ({ ok: true, reply: '' }),
})

describe('History', () => {
  it('shows a plain sentence per row, with the task title, and no raw data by default', async () => {
    const { container } = render(History, { client: client(), repo: 'r1' })
    expect(await screen.findByText('Task 4 (Login form) passed its tests')).toBeInTheDocument()
    expect(screen.getByText('Task 4 (Login form) claimed by session a1b2c3')).toBeInTheDocument()
    // An unknown kind reads as its name in words.
    expect(screen.getByText('Brand new thing', { selector: 'span' })).toBeInTheDocument()
    expect(container.querySelector('[data-testid="raw"]')).toBeNull()
    expect(container.textContent).not.toContain('verif_id')
    // No code reaches the sentences.
    const text = [...container.querySelectorAll('li')].map((l) => l.textContent).join(' ')
    for (const w of jargon.words) expect(text.toLowerCase()).not.toContain(w)
  })

  it('reveals the raw details only when asked', async () => {
    const { container } = render(History, { client: client(), repo: 'r1' })
    const btn = await screen.findByRole('button', { name: 'Show details of event 3' })
    expect(btn).toHaveAttribute('aria-expanded', 'false')
    await fireEvent.click(btn)
    const raw = container.querySelector('[data-testid="raw"]')!
    expect(raw.textContent).toContain('verif_id')
    await fireEvent.click(screen.getByRole('button', { name: 'Hide details of event 3' }))
    expect(container.querySelector('[data-testid="raw"]')).toBeNull()
  })

  it('filters by a menu of the kinds present, not free text', async () => {
    render(History, { client: client(), repo: 'r1' })
    const menu = await screen.findByRole('combobox', { name: 'kind of event' })
    const labels = [...menu.querySelectorAll('option')].map((o) => o.textContent)
    expect(labels).toEqual([
      'All kinds of event',
      'Brand new thing',
      'Checked and passed',
      'Task claimed',
    ])
  })
})

describe('historyText', () => {
  it('falls back to the kind in words, and joins a task number with its title', () => {
    expect(kindWords('brand_new_thing')).toBe('Brand new thing')
    expect(sentence({ kind: 'plan_edited', summary: '' })).toBe('Plan edited')
    expect(sentence({ kind: 'x', summary: 'Said so' })).toBe('Said so')
    expect(taskText({ task_id: '3.1', task_shown: '1.1', task_title: 'Login' })).toBe('1.1 Login')
    expect(taskText({ task_id: null })).toBeNull()
    expect(kindOptions(undefined)).toEqual([])
  })
})
