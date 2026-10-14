// The MCP Apps view side (ext-apps spec 2026-01-26): JSON-RPC over postMessage with the
// host. `ui/initialize` → host context (theme, style variables) → `ui/notifications/
// initialized`; the tool result arrives as `ui/notifications/tool-result`; buttons call
// server tools with `tools/call`; the card reports its size with
// `ui/notifications/size-changed`.

export interface ToolResult {
  content?: { type: string; text?: string }[]
  structuredContent?: unknown
  isError?: boolean
  _meta?: Record<string, unknown>
}

export interface HostContext {
  theme?: 'light' | 'dark'
  styles?: { variables?: Record<string, string> }
}

export interface CardHost {
  /** Handshake; resolves with the host context. */
  start(): Promise<HostContext>
  onToolResult(cb: (r: ToolResult) => void): void
  onHostContext(cb: (c: HostContext) => void): void
  callTool(name: string, args: Record<string, unknown>): Promise<ToolResult>
  sizeChanged(width: number, height: number): void
}

export const PROTOCOL_VERSION = '2026-01-26'

/** Host style variables mapped onto the palette's surface and text tokens. */
const HOST_VARS: Record<string, string> = {
  '--color-background-primary': '--bg',
  '--color-background-secondary': '--surface',
  '--color-text-primary': '--fg',
  '--color-text-secondary': '--muted',
  '--color-border-primary': '--line',
}

/**
 * The theme comes from the host, never from prefers-color-scheme (which is unreliable
 * inside a sandboxed frame): `data-theme` is always set, dark when the host says nothing.
 */
export function applyTheme(
  ctx: HostContext | undefined,
  root: HTMLElement = document.documentElement,
) {
  root.dataset.theme = ctx?.theme === 'light' ? 'light' : 'dark'
  for (const [name, value] of Object.entries(ctx?.styles?.variables ?? {})) {
    if (typeof value !== 'string') continue
    root.style.setProperty(name, value)
    const ours = HOST_VARS[name]
    if (ours) root.style.setProperty(ours, value)
  }
}

type Pending = { resolve: (v: unknown) => void; reject: (e: Error) => void }

/** The real host: the parent window. */
export class PostMessageHost implements CardHost {
  private next = 1
  private pending = new Map<number, Pending>()
  private results: ((r: ToolResult) => void)[] = []
  private contexts: ((c: HostContext) => void)[] = []

  constructor(
    private target: Window = window.parent,
    private source: Window = window,
  ) {
    this.source.addEventListener('message', (e: MessageEvent) => this.receive(e.data))
  }

  private post(msg: object) {
    this.target.postMessage({ jsonrpc: '2.0', ...msg }, '*')
  }

  private request(method: string, params: object): Promise<unknown> {
    const id = this.next++
    this.post({ id, method, params })
    return new Promise((resolve, reject) => this.pending.set(id, { resolve, reject }))
  }

  private receive(msg: unknown) {
    if (!msg || typeof msg !== 'object') return
    const m = msg as {
      id?: number
      method?: string
      params?: unknown
      result?: unknown
      error?: { message?: string }
    }
    if (m.id !== undefined && !m.method) {
      const p = this.pending.get(m.id)
      if (!p) return
      this.pending.delete(m.id)
      if (m.error) p.reject(new Error(m.error.message ?? 'request failed'))
      else p.resolve(m.result)
      return
    }
    if (m.method === 'ui/notifications/tool-result') {
      for (const cb of this.results) cb(m.params as ToolResult)
    } else if (m.method === 'ui/notifications/host-context-changed') {
      for (const cb of this.contexts) cb(m.params as HostContext)
    }
  }

  async start(): Promise<HostContext> {
    const result = (await this.request('ui/initialize', {
      protocolVersion: PROTOCOL_VERSION,
      clientInfo: { name: 'bridge-screen progress', version: '1' },
      appCapabilities: {},
    })) as { hostContext?: HostContext } | undefined
    this.post({ method: 'ui/notifications/initialized', params: {} })
    return result?.hostContext ?? {}
  }

  onToolResult(cb: (r: ToolResult) => void) {
    this.results.push(cb)
  }

  onHostContext(cb: (c: HostContext) => void) {
    this.contexts.push(cb)
  }

  async callTool(name: string, args: Record<string, unknown>): Promise<ToolResult> {
    return (await this.request('tools/call', { name, arguments: args })) as ToolResult
  }

  sizeChanged(width: number, height: number) {
    this.post({ method: 'ui/notifications/size-changed', params: { width, height } })
  }
}
