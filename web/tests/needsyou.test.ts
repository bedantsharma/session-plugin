import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import userEvent from '@testing-library/user-event'
import { describe, expect, it, vi } from 'vitest'
import NeedsYou from '../src/lib/glance/NeedsYou.svelte'
import type { ActResult } from '../src/lib/api'
import { item, leftover, question } from './fixtures'

function deferred() {
  let resolve!: (r: ActResult) => void
  const promise = new Promise<ActResult>((r) => (resolve = r))
  return { promise, resolve }
}

describe('NeedsYou', () => {
  it('renders the server actions and sends their JSON as given', async () => {
    const act = vi.fn(async () => ({ ok: true, reply: 'Told s1aaaa: stop.' }) as ActResult)
    render(NeedsYou, { items: [item()], act })
    expect(screen.getByRole('button', { name: 'Continue' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Make it a task' })).toBeInTheDocument()
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }))
    expect(act).toHaveBeenCalledWith({ kind: 'drift', session: 's1aaaa', answer: 'stop' })
    expect(await screen.findByText('Told s1aaaa: stop.')).toBeInTheDocument()
  })

  it('Continue sends an optional inline reason (empty is still one click)', async () => {
    const act = vi.fn(async () => ({ ok: true, reply: 'ok' }) as ActResult)
    render(NeedsYou, { items: [item()], act })
    await userEvent.click(screen.getByRole('button', { name: 'Continue' }))
    expect(act).toHaveBeenLastCalledWith({ kind: 'drift', session: 's1aaaa', answer: 'continue' })
    await userEvent.type(
      screen.getByLabelText('Continue: reason (optional)'),
      'the CI fix unblocks the release',
    )
    await userEvent.click(screen.getByRole('button', { name: 'Continue' }))
    expect(act).toHaveBeenLastCalledWith({
      kind: 'drift',
      session: 's1aaaa',
      answer: 'continue',
      reason: 'the CI fix unblocks the release',
    })
  })

  it('shows an invalid-reason hint or a 409 without removing the item', async () => {
    const act = vi.fn(
      async () =>
        ({
          ok: false,
          status: 409,
          error: 'already resolved: the owner already answered',
        }) as ActResult,
    )
    render(NeedsYou, { items: [item()], act })
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }))
    expect(await screen.findByText(/already resolved/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Stop' })).toBeInTheDocument()
  })

  it('disables the buttons while the server answers', async () => {
    const d = deferred()
    const act = vi.fn(() => d.promise)
    render(NeedsYou, { items: [item()], act })
    await userEvent.click(screen.getByRole('button', { name: 'Stop' }))
    const stop = screen.getByRole('button', { name: 'Stop…' })
    expect(stop).toBeDisabled()
    expect(screen.getByRole('button', { name: 'Continue' })).toBeDisabled()
    await userEvent.click(stop)
    expect(act).toHaveBeenCalledTimes(1)
    d.resolve({ ok: true, reply: 'done' })
    await waitFor(() => expect(screen.getByRole('button', { name: 'Stop' })).toBeEnabled())
  })

  it('Undo is one click, no dialog, and says how to get the files back', async () => {
    const reply =
      'Undid 1 leftover: stashed as abc (`git stash apply abc` in /wt brings them back).'
    const act = vi.fn(async () => ({ ok: true, reply }) as ActResult)
    render(NeedsYou, { items: [leftover()], act })
    await userEvent.click(screen.getByRole('button', { name: 'Undo' }))
    expect(screen.queryByRole('dialog')).toBeNull()
    expect(act).toHaveBeenCalledWith({
      kind: 'resolve',
      repo: 'r1',
      leftover_ids: ['lab'],
      action: 'undo',
    })
    expect(await screen.findByText(/git stash apply abc/)).toBeInTheDocument()
  })

  it('Make it a task asks for its inputs in a dialog first', async () => {
    const act = vi.fn(async () => ({ ok: true, reply: 'ok' }) as ActResult)
    render(NeedsYou, { items: [item()], act })
    await userEvent.click(screen.getByRole('button', { name: 'Make it a task' }))
    expect(act).not.toHaveBeenCalled()
    const dialog = await screen.findByRole('dialog')
    expect(dialog).toBeInTheDocument()
    await userEvent.type(screen.getByLabelText('Task title'), 'Fix the CI cache')
    await userEvent.click(screen.getAllByRole('button', { name: 'Make it a task' }).at(-1)!)
    expect(act).toHaveBeenCalledWith({
      kind: 'drift',
      session: 's1aaaa',
      answer: 'make_task',
      parent: '1',
      title: 'Fix the CI cache',
    })
  })

  it('items without buttons say where to answer', () => {
    const { container } = render(NeedsYou, { items: [question('abc123')], act: vi.fn() })
    expect(container.querySelector('[data-item] .font-mono')).toHaveTextContent('S abc123')
    expect(screen.getByText('/wt/feat-x')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Copy the worktree path' })).toBeInTheDocument()
  })

  it('does not reorder or remove rows while the pointer is inside', async () => {
    const a = item({ id: 'a', title: 'first' })
    const b = question('b')
    const { rerender, container } = render(NeedsYou, { items: [a, b], act: vi.fn() })
    const section = screen.getByRole('region', { name: 'Needs you' })
    await fireEvent.pointerEnter(section)
    await rerender({ items: [b], act: vi.fn() })
    const ids = () =>
      [...container.querySelectorAll('[data-item]')].map((e) => e.getAttribute('data-item'))
    expect(ids()).toEqual(['a', 'question:b'])
    await fireEvent.pointerLeave(section)
    await waitFor(() => expect(ids()).toEqual(['question:b']))
  })

  it('shows the 4 most urgent, and all on request', async () => {
    const many = [0, 1, 2, 3, 4, 5].map((i) => question(`s${i}`))
    render(NeedsYou, { items: many, act: vi.fn() })
    expect(screen.getAllByText(/asked you a question/)).toHaveLength(4)
    await fireEvent.click(screen.getByRole('button', { name: 'show all 6' }))
    expect(screen.getAllByText(/asked you a question/)).toHaveLength(6)
  })
})
