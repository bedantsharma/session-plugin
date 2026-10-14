import { render, screen, waitFor } from '@testing-library/svelte'
import userEvent from '@testing-library/user-event'
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { describe, expect, it, vi } from 'vitest'
import Card from '../src/card/Card.svelte'
import {
  PostMessageHost,
  applyTheme,
  type CardHost,
  type HostContext,
  type ToolResult,
} from '../src/card/host'
import { glance, item, question } from './fixtures'

class FakeHost implements CardHost {
  resultCb: ((r: ToolResult) => void) | null = null
  sizes: [number, number][] = []
  calls: [string, Record<string, unknown>][] = []
  replies: ToolResult[] = []
  constructor(private ctx: HostContext = {}) {}
  async start() {
    return this.ctx
  }
  onToolResult(cb: (r: ToolResult) => void) {
    this.resultCb = cb
  }
  onHostContext() {}
  async callTool(name: string, args: Record<string, unknown>) {
    this.calls.push([name, args])
    return (
      this.replies.shift() ?? {
        structuredContent: glance([]),
        _meta: { 'bridge-screen/nonce': 'n2' },
      }
    )
  }
  sizeChanged(w: number, h: number) {
    this.sizes.push([w, h])
  }
  push(r: ToolResult) {
    this.resultCb?.(r)
  }
}

const result = (g = glance([item()])): ToolResult => ({
  content: [{ type: 'text', text: 'status' }],
  structuredContent: g,
  _meta: { 'bridge-screen/nonce': 'n1' },
})

describe('desktop card', () => {
  it('renders the glance from the tool result and reports its size', async () => {
    const host = new FakeHost()
    render(Card, { host })
    host.push(result())
    expect(await screen.findByText('S s1aaaa is far off its task')).toBeInTheDocument()
    expect(screen.getByTestId('pill')).toHaveTextContent('1 need you')
    expect(screen.getByText(/as of/)).toBeInTheDocument()
    await waitFor(() => expect(host.sizes.length).toBeGreaterThan(0))
  })

  it('shows at most 4 items and points to the dashboard for the rest', async () => {
    const host = new FakeHost()
    render(Card, { host })
    host.push(result(glance([0, 1, 2, 3, 4, 5].map((i) => question(`s${i}`)))))
    expect(await screen.findAllByText(/asked you a question/)).toHaveLength(4)
    expect(screen.getByText(/2 more — open the dashboard/)).toBeInTheDocument()
  })

  it('a click calls owner_action with the nonce, then refreshes', async () => {
    const host = new FakeHost()
    host.replies = [
      { content: [{ type: 'text', text: 'Told s1aaaa: stop.' }] },
      { structuredContent: glance([]), _meta: { 'bridge-screen/nonce': 'n2' } },
    ]
    render(Card, { host })
    host.push(result())
    await userEvent.click(await screen.findByRole('button', { name: 'Stop' }))
    expect(host.calls[0]).toEqual([
      'owner_action',
      { action: { kind: 'drift', session: 's1aaaa', answer: 'stop' }, nonce: 'n1' },
    ])
    expect(host.calls[1][0]).toBe('show_progress')
    await waitFor(() => expect(screen.getByText('● All clear')).toBeInTheDocument())
  })

  it('shows a refusal without refreshing it away', async () => {
    const host = new FakeHost()
    host.replies = [{ isError: true, content: [{ type: 'text', text: 'already resolved: x' }] }]
    render(Card, { host })
    host.push(result())
    await userEvent.click(await screen.findByRole('button', { name: 'Stop' }))
    expect(await screen.findByText('already resolved: x')).toBeInTheDocument()
    expect(host.calls).toHaveLength(1)
  })

  it('keeps milestones behind "show milestones", with the hill in words', async () => {
    const host = new FakeHost()
    const { container } = render(Card, { host })
    host.push(result())
    const toggle = await screen.findByRole('button', { name: 'show milestones' })
    expect(container.querySelector('[data-milestone]')).toBeNull()
    await userEvent.click(toggle)
    expect(container.querySelector('[data-milestone="1"]')).toHaveTextContent('exploring')
    expect(container.querySelector('[data-milestone="2"]')).toHaveTextContent('executing')
    expect(container.textContent).not.toMatch(/uphill|downhill/)
  })

  it('has its own Motion switch', async () => {
    const host = new FakeHost()
    render(Card, { host })
    host.push(result())
    const btn = await screen.findByRole('button', { name: 'Motion' })
    const before = btn.getAttribute('aria-pressed')
    await userEvent.click(btn)
    expect(btn.getAttribute('aria-pressed')).not.toBe(before)
  })

  it('Refresh re-calls show_progress', async () => {
    const host = new FakeHost()
    render(Card, { host })
    host.push(result())
    await userEvent.click(await screen.findByRole('button', { name: 'Refresh' }))
    expect(host.calls[0][0]).toBe('show_progress')
  })
})

describe('host bridge', () => {
  it('the theme comes from the host, dark when it says nothing', () => {
    const root = document.createElement('html')
    applyTheme(undefined, root)
    expect(root.dataset.theme).toBe('dark')
    applyTheme(
      { theme: 'light', styles: { variables: { '--color-text-primary': '#111111' } } },
      root,
    )
    expect(root.dataset.theme).toBe('light')
    expect(root.style.getPropertyValue('--fg')).toBe('#111111')
  })

  it('speaks the MCP Apps protocol over postMessage', async () => {
    const sent: Record<string, unknown>[] = []
    const target = {
      postMessage: (m: Record<string, unknown>) => void sent.push(m),
    } as unknown as Window
    const listeners: ((e: MessageEvent) => void)[] = []
    const source = {
      addEventListener: (_: string, cb: (e: MessageEvent) => void) => void listeners.push(cb),
    } as unknown as Window
    const host = new PostMessageHost(target, source)
    const started = host.start()
    expect(sent[0]).toMatchObject({ jsonrpc: '2.0', method: 'ui/initialize', id: 1 })
    expect((sent[0].params as { protocolVersion: string }).protocolVersion).toBe('2026-01-26')
    for (const l of listeners)
      l({
        data: { jsonrpc: '2.0', id: 1, result: { hostContext: { theme: 'light' } } },
      } as MessageEvent)
    expect(await started).toEqual({ theme: 'light' })
    expect(sent[1]).toMatchObject({ method: 'ui/notifications/initialized' })
    host.sizeChanged(300, 200)
    expect(sent[2]).toEqual({
      jsonrpc: '2.0',
      method: 'ui/notifications/size-changed',
      params: { width: 300, height: 200 },
    })
    const got = vi.fn()
    host.onToolResult(got)
    for (const l of listeners)
      l({
        data: {
          jsonrpc: '2.0',
          method: 'ui/notifications/tool-result',
          params: { structuredContent: 1 },
        },
      } as MessageEvent)
    expect(got).toHaveBeenCalledWith({ structuredContent: 1 })
    const call = host.callTool('show_progress', {})
    expect(sent[3]).toMatchObject({
      method: 'tools/call',
      params: { name: 'show_progress', arguments: {} },
    })
    for (const l of listeners)
      l({ data: { jsonrpc: '2.0', id: 2, result: { isError: false } } } as MessageEvent)
    expect(await call).toEqual({ isError: false })
  })

  it('the card build styles by data-theme, not by prefers-color-scheme alone', () => {
    const main = readFileSync(resolve(process.cwd(), 'src/card/main.ts'), 'utf8')
    expect(main).toContain('applyTheme(undefined)')
    const css = readFileSync(resolve(process.cwd(), 'src/app.css'), 'utf8')
    // The media query only applies when no data-theme says dark, and the card always sets one.
    expect(css).toContain(":root:not([data-theme='dark'])")
    expect(css).toContain(":root[data-theme='light']")
  })
})
