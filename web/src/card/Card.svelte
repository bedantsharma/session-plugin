<script lang="ts">
  // The desktop card (spec §10.5): the glance from show_progress's structured content —
  // at most 4 needs-you items with their buttons, project bars with text percentages,
  // session lights with words. No polling (a card sits in the transcript): it says
  // "as of HH:MM" and has a Refresh button. Clicks go back as owner_action calls with the
  // nonce show_progress gave it.
  import { onMount, tick } from 'svelte'
  import ProgressBar from '$lib/glance/ProgressBar.svelte'
  import { hhmm } from '$lib/format'
  import type { ActionSpec, Glance, NeedsYou } from '$lib/types'
  import { applyTheme, type CardHost, type ToolResult } from './host'

  let { host }: { host: CardHost } = $props()

  const NONCE_KEY = 'bridge-screen/nonce'
  const URGENT = 4

  let glance = $state<Glance | null>(null)
  let nonce = $state<string | null>(null)
  let pending = $state<Record<string, string | null>>({})
  let messages = $state<Record<string, { ok: boolean; text: string }>>({})
  let inputs = $state<Record<string, string>>({})
  let refreshing = $state(false)
  let root = $state<HTMLElement | null>(null)

  function take(r: ToolResult) {
    if (r.structuredContent && typeof r.structuredContent === 'object') {
      glance = r.structuredContent as Glance
    }
    const n = r._meta?.[NONCE_KEY]
    if (typeof n === 'string') nonce = n
  }

  async function refresh() {
    refreshing = true
    try {
      take(await host.callTool('show_progress', {}))
    } finally {
      refreshing = false
    }
  }

  function missing(item: NeedsYou, spec: ActionSpec): boolean {
    return spec.inputs.some((i) => i.required && !(inputs[`${item.id}:${i.name}`] ?? '').trim())
  }

  async function click(item: NeedsYou, spec: ActionSpec) {
    const extra: Record<string, string> = {}
    for (const i of spec.inputs) {
      const v = (inputs[`${item.id}:${i.name}`] ?? '').trim()
      if (v) extra[i.name] = v
    }
    pending[item.id] = spec.label
    delete messages[item.id]
    try {
      const r = await host.callTool('owner_action', {
        action: { ...spec.action, ...extra },
        nonce,
      })
      const text = r.content?.find((c) => c.type === 'text')?.text ?? ''
      messages[item.id] = { ok: !r.isError, text }
      if (!r.isError) await refresh()
    } catch (e) {
      messages[item.id] = { ok: false, text: (e as Error).message }
    } finally {
      pending[item.id] = null
    }
  }

  onMount(() => {
    host.onToolResult(take)
    host.onHostContext((c) => applyTheme(c))
    void host.start().then((c) => applyTheme(c))
  })

  // Tell the host how big the card is after each render.
  $effect(() => {
    void [glance, messages, pending]
    void tick().then(() => {
      if (root) host.sizeChanged(Math.ceil(root.scrollWidth), Math.ceil(root.scrollHeight))
    })
  })

  const sessions = $derived(glance?.projects.flatMap((p) => p.sessions) ?? [])
  const shape: Record<string, string> = { green: '●', amber: '◆', red: '■' }
  const colour: Record<string, string> = { green: 'text-ok', amber: 'text-warn', red: 'text-bad' }
</script>

<div bind:this={root} class="flex flex-col gap-2 bg-bg p-3 text-sm text-fg">
  {#if !glance}
    <p class="text-muted">Loading progress…</p>
  {:else}
    <header class="flex items-center gap-2">
      <span class="text-clay" aria-hidden="true">✱</span>
      <span class="font-medium">bridge-screen</span>
      <span class="text-meta text-muted">as of {hhmm(glance.as_of)}</span>
      <span class="ml-auto">
        {#if glance.needs_you_count > 0}
          <span
            class="rounded-full border-2 border-bad px-2 text-meta font-medium text-bad"
            data-testid="pill">■ {glance.needs_you_count} need you</span
          >
        {:else}
          <span class="rounded-full border border-ok px-2 text-meta text-ok">● All clear</span>
        {/if}
      </span>
      <button
        type="button"
        class="rounded border border-edge px-2 text-meta"
        disabled={refreshing}
        onclick={refresh}>{refreshing ? 'Refreshing…' : 'Refresh'}</button
      >
    </header>
    {#if glance.needs_you.length}
      <ul class="flex flex-col gap-1 rounded-md border-2 border-bad p-2" aria-label="Needs you">
        {#each glance.needs_you.slice(0, URGENT) as item (item.id)}
          <li class="flex flex-col gap-1" data-item={item.id}>
            <div>{item.title} <span class="text-meta text-muted">· {item.repo_name}</span></div>
            <div class="text-meta text-muted">{item.detail}</div>
            {#if item.actions.length}
              <div class="flex flex-wrap items-center gap-1">
                {#each item.actions as spec (spec.label)}
                  {#each spec.inputs as inp (inp.name)}
                    <input
                      class="h-7 w-32 rounded border border-edge bg-surface px-1 text-meta"
                      placeholder={inp.label}
                      aria-label={`${spec.label}: ${inp.label}`}
                      bind:value={inputs[`${item.id}:${inp.name}`]}
                    />
                  {/each}
                  <button
                    type="button"
                    class={[
                      'h-7 rounded border px-2 text-meta',
                      spec.tone === 'danger' ? 'border-bad text-bad' : 'border-edge',
                    ]}
                    disabled={!!pending[item.id] || missing(item, spec)}
                    onclick={() => click(item, spec)}
                    >{pending[item.id] === spec.label ? `${spec.label}…` : spec.label}</button
                  >
                {/each}
              </div>
            {:else if item.respond_in}
              <div class="text-meta text-muted">
                answer in <span class="font-mono">S {item.respond_in.session_short}</span>
                {#if item.respond_in.worktree}· <span class="font-mono"
                    >{item.respond_in.worktree}</span
                  >{/if}
              </div>
            {/if}
            {#if messages[item.id]}
              <div
                class={['text-meta', messages[item.id].ok ? 'text-ok' : 'text-warn']}
                role="status"
              >
                {messages[item.id].text}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
      {#if glance.needs_you.length > URGENT}
        <p class="text-meta text-muted">
          {glance.needs_you.length - URGENT} more — open the dashboard (<span class="font-mono"
            >bridge-screen ui</span
          >)
        </p>
      {/if}
    {/if}
    <ul class="flex flex-col gap-1">
      {#each glance.projects as p (p.repo_id)}
        <li>
          <div class="flex justify-between text-meta">
            <span class="font-medium">{p.name}</span>
            <span
              >{p.done_pct}% done{#if p.claimed_pct > 0}&nbsp;· +{p.claimed_pct}% claimed{/if}</span
            >
          </div>
          <ProgressBar done={p.done_pct} claimed={p.claimed_pct} />
        </li>
      {/each}
    </ul>
    <ul class="flex flex-col">
      {#each sessions as s (s.session_id)}
        <li class="flex gap-2 text-meta">
          <span class={['w-20', colour[s.light]]}>{shape[s.light]} {s.light_word}</span>
          <span class="font-mono">S {s.short}</span>
          <span class="truncate">{s.task ? `${s.task.id} ${s.task.title}` : ''}</span>
        </li>
      {/each}
    </ul>
  {/if}
</div>
