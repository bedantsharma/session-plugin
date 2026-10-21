<script lang="ts">
  // The desktop card (spec §10.5): the glance from show_progress's structured content —
  // at most 4 needs-you items with their buttons, project bars with text percentages,
  // session lights with words; milestones behind "show milestones" (plan §2, desktop
  // card). No polling (a card sits in the transcript): it says
  // "as of HH:MM" and has a Refresh button. Clicks go back as owner_action calls with the
  // nonce show_progress gave it.
  import { onMount, tick } from 'svelte'
  import ActionButton from '$lib/glance/ActionButton.svelte'
  import ProgressBar from '$lib/glance/ProgressBar.svelte'
  import Chip from '$lib/ui/Chip.svelte'
  import { changed } from '$lib/changed'
  import { hhmm } from '$lib/format'
  import { motion } from '$lib/motion.svelte'
  import { milestoneHill, rollupWord, sessionWord } from '$lib/words'
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
  let openMilestones = $state<Record<string, boolean>>({})
  /** Per item: the action whose form is open (one that needs inputs, e.g. "Make it a task"). */
  let form = $state<Record<string, string | null>>({})

  /** An action that needs answers first opens its own row instead of sending. */
  const needsForm = (spec: ActionSpec) => spec.inputs.some((i) => i.required)
  const submitLabel = (spec: ActionSpec) =>
    spec.action.answer === 'make_task' ? 'Create task' : spec.label

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
      if (!r.isError) {
        form[item.id] = null
        await refresh()
      }
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
    void [glance, messages, pending, openMilestones, form]
    void tick().then(() => {
      if (root) host.sizeChanged(Math.ceil(root.scrollWidth), Math.ceil(root.scrollHeight))
    })
  })

  const sessions = $derived(glance?.projects.flatMap((p) => p.sessions) ?? [])
</script>

<div bind:this={root} class="card-root flex flex-col gap-3 bg-bg p-4 text-sm text-fg">
  {#if !glance}
    <p class="text-muted">Loading progress…</p>
  {:else}
    <header class="flex h-8 items-center gap-2">
      <span class="text-clay" aria-hidden="true">✱</span>
      <span class="font-semibold">bridge-screen</span>
      <span class="text-xs text-muted tabular-nums">as of {hhmm(glance.as_of)}</span>
      <span class="ml-auto">
        {#if glance.needs_you_count > 0}
          <span
            class="inline-flex h-7 items-center rounded-full border-[1.5px] border-bad bg-tint-bad px-3 text-xs font-medium text-bad"
            data-testid="pill">■ {glance.needs_you_count} need you</span
          >
        {:else}
          <span
            class="inline-flex h-7 items-center rounded-full border-[1.5px] border-ok px-3 text-xs font-medium text-ok"
            >● All clear</span
          >
        {/if}
      </span>
      <button
        type="button"
        class="grid size-8 place-items-center rounded-lg border border-edge text-muted hover:bg-hover hover:text-fg"
        aria-pressed={motion.on}
        aria-label="Motion"
        title={motion.on ? 'Motion on — click to pause animations' : 'Motion off — click to resume'}
        onclick={() => motion.toggle()}
      >
        <svg viewBox="0 0 16 16" class="size-4" aria-hidden="true" fill="currentColor">
          {#if motion.on}
            <rect x="4" y="3" width="2.5" height="10" rx="0.5" />
            <rect x="9.5" y="3" width="2.5" height="10" rx="0.5" />
          {:else}
            <path
              d="M5 3.2v9.6a.5.5 0 0 0 .77.42l7.2-4.8a.5.5 0 0 0 0-.84l-7.2-4.8A.5.5 0 0 0 5 3.2z"
            />
          {/if}
        </svg>
      </button>
      <button
        type="button"
        class="inline-flex h-8 items-center rounded-lg border border-edge px-3 text-sm font-medium hover:bg-hover disabled:opacity-50"
        disabled={refreshing}
        onclick={refresh}>{refreshing ? 'Refreshing…' : 'Refresh'}</button
      >
    </header>
    {#if glance.needs_you.length}
      <ul class="flex flex-col gap-2" aria-label="Needs you">
        {#each glance.needs_you.slice(0, URGENT) as item (item.id)}
          <li
            class="flex flex-col gap-2 rounded-xl border border-l-[3px] border-line border-l-bad bg-surface p-4"
            data-item={item.id}
          >
            <div class="flex flex-col gap-0.5">
              <div class="flex items-center gap-2">
                <span class="min-w-0 flex-1 truncate font-medium">{item.title}</span>
                <span class="shrink-0 text-xs text-muted">{item.repo_name}</span>
              </div>
              <div class="line-clamp-2 text-xs text-muted">{item.detail}</div>
            </div>
            {#if item.actions.length}
              {@const open = item.actions.find((a) => a.label === form[item.id])}
              <div class="flex flex-wrap items-center gap-2">
                {#each item.actions as spec (spec.label)}
                  {#if needsForm(spec)}
                    <ActionButton
                      label={spec.label}
                      tone={spec.tone}
                      compact
                      disabled={!!pending[item.id] || open === spec}
                      onclick={() => (form[item.id] = spec.label)}
                    />
                  {:else}
                    {#each spec.inputs as inp (inp.name)}
                      <input
                        class="h-8 w-40 min-w-0 rounded-lg border border-edge bg-surface px-2 text-sm"
                        placeholder={inp.label}
                        aria-label={`${spec.label}: ${inp.label}`}
                        bind:value={inputs[`${item.id}:${inp.name}`]}
                      />
                    {/each}
                    <ActionButton
                      label={spec.label}
                      tone={spec.tone}
                      compact
                      disabled={!!pending[item.id]}
                      pending={pending[item.id] === spec.label}
                      onclick={() => click(item, spec)}
                    />
                  {/if}
                {/each}
              </div>
              {#if open}
                <div class="flex w-full flex-wrap items-center gap-2" data-testid="task-form">
                  {#each open.inputs as inp (inp.name)}
                    <input
                      class="h-8 min-w-32 flex-1 rounded-lg border border-edge bg-surface px-2 text-sm"
                      placeholder={inp.label}
                      aria-label={`${open.label}: ${inp.label}`}
                      bind:value={inputs[`${item.id}:${inp.name}`]}
                    />
                  {/each}
                  <ActionButton
                    label={submitLabel(open)}
                    tone="primary"
                    compact
                    disabled={!!pending[item.id] || missing(item, open)}
                    pending={pending[item.id] === open.label}
                    onclick={() => click(item, open)}
                  />
                  <ActionButton
                    label="Cancel"
                    tone="neutral"
                    compact
                    disabled={!!pending[item.id]}
                    onclick={() => (form[item.id] = null)}
                  />
                </div>
              {/if}
            {:else if item.respond_in}
              <div class="text-xs text-muted">
                answer in <span class="font-mono">S {item.respond_in.session_short}</span>
                {#if item.respond_in.worktree}· <span class="font-mono"
                    >{item.respond_in.worktree}</span
                  >{/if}
              </div>
            {/if}
            {#if messages[item.id]}
              <div
                class={['text-xs', messages[item.id].ok ? 'text-ok' : 'text-warn']}
                role="status"
              >
                {messages[item.id].text}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
      {#if glance.needs_you.length > URGENT}
        <p class="text-xs text-muted">
          {glance.needs_you.length - URGENT} more — open the dashboard (<span class="font-mono"
            >bridge-screen ui</span
          >)
        </p>
      {/if}
    {/if}
    <ul class="flex flex-col rounded-xl border border-line bg-surface px-4" aria-label="Projects">
      {#each glance.projects as p (p.repo_id)}
        <li class="border-b border-line py-1 last:border-b-0" data-project={p.repo_id}>
          <div
            class="grid min-h-10 grid-cols-[minmax(0,10rem)_minmax(0,1fr)_auto] items-center gap-3"
          >
            <span class="truncate font-medium">{p.name}</span>
            <ProgressBar done={p.done_pct} claimed={p.claimed_pct} size="card" />
            <span class="text-right tabular-nums"
              >{p.done_pct}% done{#if p.claimed_pct > 0}<span class="text-xs text-muted"
                  >&nbsp;· +{p.claimed_pct}% claimed</span
                >{/if}</span
            >
          </div>
          {#if p.forecast || p.cost}
            <p class="pb-1 text-xs text-muted tabular-nums" data-testid="card-forecast">
              {#if p.forecast}<span title={p.forecast.help}>{p.forecast.short}</span><span
                  class="sr-only">{` (${p.forecast.help})`}</span
                >{/if}{#if p.forecast && p.cost}<span>&nbsp;·&nbsp;</span>{/if}{#if p.cost}<span
                  >{p.cost.text}</span
                >{/if}
            </p>
          {/if}
          {#if p.milestones.length}
            <button
              type="button"
              class="mb-1 rounded-md px-1 text-xs text-muted hover:bg-hover hover:text-fg"
              aria-expanded={!!openMilestones[p.repo_id]}
              onclick={() => (openMilestones[p.repo_id] = !openMilestones[p.repo_id])}
              >{openMilestones[p.repo_id] ? 'hide milestones' : 'show milestones'}</button
            >
            {#if openMilestones[p.repo_id]}
              <ul class="grid grid-cols-[2rem_minmax(0,1fr)_5rem_2.5rem_auto] gap-x-2 pb-2">
                {#each p.milestones as m (m.id)}
                  {@const hill = milestoneHill(m)}
                  {@const roll = rollupWord(m.status)}
                  <li
                    class="col-span-5 grid min-h-8 grid-cols-subgrid items-center text-xs"
                    data-milestone={m.id}
                  >
                    <span class="font-mono text-muted">{m.id}</span>
                    <span class="truncate text-sm">{m.title}</span>
                    <ProgressBar
                      done={m.done_pct}
                      claimed={m.claimed_pct}
                      size="mini"
                      labelled={false}
                    />
                    <span class={['text-right tabular-nums', m.hill === 'uphill' && 'text-muted']}
                      >{m.done_pct}%</span
                    >
                    <span class="flex gap-1">
                      {#if roll}<Chip tone={roll.tone} icon={roll.icon} text={roll.text} />{/if}
                      {#if hill}<Chip
                          tone={hill.tone}
                          icon={hill.icon}
                          text={hill.text}
                          tip={hill.tip}
                        />{/if}
                      {#if m.stuck}<Chip tone="warn" icon="!" text="stuck" />{/if}
                    </span>
                  </li>
                {/each}
              </ul>
            {/if}
          {/if}
        </li>
      {/each}
    </ul>
    {#if sessions.length}
      <ul class="flex flex-col rounded-xl border border-line bg-surface px-2" aria-label="Sessions">
        {#each sessions as s (s.session_id)}
          {@const w = sessionWord(s)}
          <li
            class={[
              'flex min-h-10 items-center gap-2 rounded-lg border-l-2 px-2',
              s.state_label === 'working' ? 'border-l-ok bg-tint-working' : 'border-l-transparent',
            ]}
            data-session={s.session_id}
            use:changed={{ value: [s.state_label, s.light_word, s.task?.id, s.detour] }}
          >
            <Chip tone={w.tone} icon={w.icon} text={w.text} live={s.state_label === 'working'} />
            <span class="font-mono text-xs">S {s.short}</span>
            <span class="min-w-0 flex-1 truncate"
              >{s.detour
                ? `${s.detour.label} ${s.detour.reason}`
                : s.task
                  ? `${s.task.id} ${s.task.title}`
                  : ''}</span
            >
            <span class="shrink-0 text-xs text-muted tabular-nums">{s.active_min} min</span>
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</div>
