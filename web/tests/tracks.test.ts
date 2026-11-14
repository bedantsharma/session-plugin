import { fireEvent, render, screen, waitFor } from '@testing-library/svelte'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'
import ProjectRow from '../src/lib/glance/ProjectRow.svelte'
import TrackBar from '../src/lib/glance/TrackBar.svelte'
import HiddenDrawer from '../src/lib/glance/HiddenDrawer.svelte'
import ConfirmRemove from '../src/lib/ui/ConfirmRemove.svelte'
import Header from '../src/lib/glance/Header.svelte'
import NeedsYou from '../src/lib/glance/NeedsYou.svelte'
import type { ActResult } from '../src/lib/api'
import { jargonIn } from '../src/lib/words'
import { glance, hiddenItem, item, project, track } from './fixtures'

// An open modal dialog sets pointer-events on the body; don't carry it into the next test.
afterEach(() => {
  document.body.style.pointerEvents = ''
})

const ok = (reply = 'ok'): ActResult => ({ ok: true, reply })
const up = { connected: true, unauthorized: false, lastOk: 0 }

describe('TrackBar', () => {
  it('trackbar_renders_caption_for_review_tasks', () => {
    render(TrackBar, { track: track(2, { reviewed_tasks: 3 }), repo: 'r1' })
    const caption = screen.getByTestId('track-caption').textContent?.replace(/\s+/g, ' ')
    expect(caption).toContain('includes 3 tasks done without tests')
    expect(screen.getByTestId('track-progress')).toHaveTextContent('10%')
  })

  it('the menu hides the bar with the stamp the screen saw', async () => {
    const act = vi.fn(async () => ok())
    render(TrackBar, { track: track(2), repo: 'r1', act })
    await userEvent.click(screen.getByRole('button', { name: 'Menu for track 2' }))
    await userEvent.click(screen.getByRole('menuitem', { name: 'Hide this track' }))
    expect(act).toHaveBeenCalledWith({
      kind: 'hide',
      repo: 'r1',
      subject: 'track:r1:2',
      label: 'Docs refresh',
      stamp: 32,
    })
  })

  it('the menu finishes a track', async () => {
    const act = vi.fn(async () => ok())
    render(TrackBar, { track: track(2), repo: 'r1', act })
    await userEvent.click(screen.getByRole('button', { name: 'Menu for track 2' }))
    await userEvent.click(screen.getByRole('menuitem', { name: 'Finish track' }))
    expect(act).toHaveBeenCalledWith({ kind: 'close_track', repo: 'r1', track: 2 })
  })
})

describe('ProjectRow with tracks', () => {
  it('projectrow_two_tracks_no_big_percent', () => {
    render(ProjectRow, { project: project({ tracks: [track(1), track(2)] }) })
    expect(screen.queryByTestId('progress-text')).toBeNull()
    expect(screen.getAllByTestId('track-progress')).toHaveLength(2)
    expect(screen.getByTestId('track-count')).toHaveTextContent('2 tracks')
  })

  it('projectrow_one_track_unchanged', () => {
    render(ProjectRow, { project: project({ tracks: [track(1)] }) })
    expect(screen.getByTestId('progress-text')).toHaveTextContent('62%')
    expect(screen.queryByTestId('track-progress')).toBeNull()
  })

  it('a stale hide shows the server message', async () => {
    const act = vi.fn(
      async () =>
        ({
          ok: false,
          status: 409,
          error: 'something new just happened, so it was not hidden; have another look',
        }) as ActResult,
    )
    render(ProjectRow, { project: project(), act })
    await userEvent.click(screen.getByRole('button', { name: 'Project menu' }))
    await userEvent.click(screen.getByRole('menuitem', { name: 'Hide this project' }))
    expect(await screen.findByText(/something new just happened/)).toBeInTheDocument()
    expect(act).toHaveBeenCalledWith({
      kind: 'hide',
      repo: 'r1',
      subject: 'project:r1',
      label: 'shop',
      stamp: 21,
    })
  })
})

describe('Hidden drawer', () => {
  it('hidden_drawer_lists_and_brings_back', async () => {
    const act = vi.fn(async () => ok('Brought back.'))
    const items = [
      hiddenItem(),
      hiddenItem({ subject: 'track:r1:2', kind: 'track', label: 'Docs refresh' }),
    ]
    render(HiddenDrawer, { items, open: true, onClose: vi.fn(), act })
    expect(screen.getByText('Check 1.2 Password reset')).toBeInTheDocument()
    expect(screen.getByText('Docs refresh')).toBeInTheDocument()
    expect(screen.getAllByText(/hidden 1h ago/)).toHaveLength(2)
    await userEvent.click(screen.getAllByRole('button', { name: 'Bring back' })[0])
    expect(act).toHaveBeenCalledWith(items[0].unhide)
  })

  it('drawer_escape_closes', async () => {
    const onClose = vi.fn()
    render(HiddenDrawer, { items: [hiddenItem()], open: true, onClose, act: vi.fn() })
    await userEvent.keyboard('{Escape}')
    await waitFor(() => expect(onClose).toHaveBeenCalled())
  })
})

describe('Remove for good', () => {
  const spec = track(2).remove!

  it('confirm_remove_shows_impact_and_needs_click', async () => {
    const act = vi.fn(async () => ok('Removed.'))
    const onClose = vi.fn()
    render(ConfirmRemove, { spec, onClose, act })
    expect(screen.getByTestId('remove-impact')).toHaveTextContent(
      "Remove the track 'Docs refresh' for good? 7 tasks, 2 done. 1 session is working on it.",
    )
    expect(act).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole('checkbox'))
    await userEvent.click(screen.getByRole('button', { name: 'Remove for good' }))
    expect(act).toHaveBeenCalledWith({ ...spec.action, confirm: true })
    await waitFor(() => expect(onClose).toHaveBeenCalled())
  })

  it('remove_button_disabled_until_confirm', async () => {
    const act = vi.fn(async () => ok())
    render(ConfirmRemove, { spec, onClose: vi.fn(), act })
    const button = screen.getByRole('button', { name: 'Remove for good' })
    expect(button).toBeDisabled()
    await userEvent.click(button)
    expect(act).not.toHaveBeenCalled()
    await fireEvent.click(screen.getByRole('checkbox'))
    expect(button).toBeEnabled()
  })

  it('keep it closes without sending anything', async () => {
    const act = vi.fn(async () => ok())
    const onClose = vi.fn()
    render(ConfirmRemove, { spec, onClose, act })
    await userEvent.click(screen.getByRole('button', { name: 'Keep it' }))
    expect(onClose).toHaveBeenCalled()
    expect(act).not.toHaveBeenCalled()
  })

  it('a stale removal says why and stays open', async () => {
    const act = vi.fn(
      async () =>
        ({ ok: false, status: 409, error: 'already gone: 1.2 was removed before' }) as ActResult,
    )
    const onClose = vi.fn()
    render(ConfirmRemove, { spec, onClose, act })
    await userEvent.click(screen.getByRole('checkbox'))
    await userEvent.click(screen.getByRole('button', { name: 'Remove for good' }))
    expect(await screen.findByRole('alert')).toHaveTextContent('already gone')
    expect(onClose).not.toHaveBeenCalled()
  })
})

describe('Hide on cards and the header', () => {
  it('needsyou_hide_sends_stamp', async () => {
    const act = vi.fn(async () => ok())
    render(NeedsYou, { items: [item()], act })
    await userEvent.click(screen.getByRole('button', { name: /^Hide:/ }))
    expect(act).toHaveBeenCalledWith({
      kind: 'hide',
      repo: 'r1',
      subject: 'card:drift_red:s1aaaa',
      label: 's1aaaa · far off its task (40 min)',
      stamp: 11,
    })
  })

  it('stale_hide_shows_server_message', async () => {
    const act = vi.fn(
      async () =>
        ({
          ok: false,
          status: 409,
          error: 'something new just happened, so it was not hidden',
        }) as ActResult,
    )
    render(NeedsYou, { items: [item()], act })
    await userEvent.click(screen.getByRole('button', { name: /^Hide:/ }))
    expect(await screen.findByText(/something new just happened/)).toBeInTheDocument()
  })

  it('header_counts_visible_and_hidden', async () => {
    const g = glance([item()])
    g.hidden = [hiddenItem(), hiddenItem({ subject: 'card:b' })]
    const onShowHidden = vi.fn()
    render(Header, { glance: g, conn: up, onShowHidden })
    expect(screen.getByTestId('pill')).toHaveTextContent('1 need you · 2 hidden')
    await userEvent.click(screen.getByTestId('hidden-button'))
    expect(onShowHidden).toHaveBeenCalled()
    expect(screen.getByTestId('hidden-button')).toHaveTextContent('Hidden (2)')
  })

  it('all clear still says what is hidden', () => {
    const g = glance([])
    g.hidden = [hiddenItem()]
    render(Header, { glance: g, conn: up })
    expect(screen.getByTestId('all-clear')).toHaveTextContent('All clear · 1 hidden')
  })
})

describe('jargon_clean_for_new_text', () => {
  it('the new screens read in words', () => {
    const a = render(TrackBar, { track: track(2, { reviewed_tasks: 1 }), repo: 'r1', act: vi.fn() })
    expect(jargonIn(a.container.textContent ?? '')).toEqual([])
    a.unmount()
    const b = render(HiddenDrawer, {
      items: [hiddenItem(), hiddenItem({ subject: 'project:r1', kind: 'project', label: 'shop' })],
      open: true,
      onClose: vi.fn(),
      act: vi.fn(),
    })
    expect(jargonIn(document.body.textContent ?? '')).toEqual([])
    b.unmount()
    const c = render(ConfirmRemove, { spec: track(2).remove, onClose: vi.fn(), act: vi.fn() })
    expect(jargonIn(document.body.textContent ?? '')).toEqual([])
    c.unmount()
  })
})
