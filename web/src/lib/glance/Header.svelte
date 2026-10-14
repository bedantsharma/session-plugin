<script lang="ts">
  // The glance header (spec §10.1, plan §2): ✱, counts, the Live indicator, the Motion
  // switch, and the one number that matters — the red "N need you" pill, or green "All
  // clear". Never green on stale data: when the server is gone or the token refused it
  // says Disconnected instead, and the Live dot gives way to a static chip.
  import type { ConnState } from '$lib/api'
  import { hhmm } from '$lib/format'
  import { motion } from '$lib/motion.svelte'
  import type { Glance } from '$lib/types'

  let {
    glance,
    conn,
    version = 0,
    onShowAll,
  }: {
    glance: Glance | null
    conn: ConnState
    /** The polling cursor: each move is a heartbeat on the Live dot. */
    version?: number
    onShowAll?: () => void
  } = $props()

  const count = $derived(glance?.needs_you_count ?? 0)
  // Pulse each time the count rises past 4 (at most 3 cycles, under 5 s; a fade under
  // reduced motion), then stay still and emphasised.
  let pulses = $state(0)
  // A small bump each time the count goes up (not on first load).
  let bumps = $state(0)
  let previous: number | null = null
  $effect(() => {
    if (count > 4 && (previous ?? 0) <= 4) pulses += 1
    if (previous !== null && count > previous) bumps += 1
    previous = count
  })
  // A heartbeat on the Live dot each time a poll brings changed data (not the first load).
  let beats = $state(0)
  let lastVersion: number | null = null
  $effect(() => {
    if (lastVersion !== null && lastVersion !== 0 && version !== lastVersion) beats += 1
    lastVersion = version
  })

  function since(t: number | null, now: number): string {
    if (t === null) return ''
    const s = Math.max(0, Math.round((now - t) / 1000))
    if (s < 60) return `updated ${s} s ago`
    return `updated ${Math.floor(s / 60)} min ago`
  }
</script>

<header class="h-16 border-b border-line bg-bg">
  <div
    class="mx-auto flex h-full w-full max-w-[1280px] items-center justify-between gap-4 px-4 lg:px-6 xl:px-8"
  >
    <h1 class="flex min-w-0 items-baseline gap-2.5">
      <span class="text-xl text-clay" aria-hidden="true">✱</span>
      <span class="text-xl font-semibold">bridge-screen</span>
      {#if glance}
        <span class="hidden truncate text-meta text-muted sm:inline">
          {glance.live_sessions}
          {glance.live_sessions === 1 ? 'session' : 'sessions'} ·
          {glance.projects.length}
          {glance.projects.length === 1 ? 'project' : 'projects'}
        </span>
      {/if}
    </h1>
    <div class="flex shrink-0 items-center gap-3">
      {#if conn.connected && glance}
        <span class="flex items-center gap-2 text-meta text-muted" data-testid="live">
          {#key beats}
            <span class={['grid size-2 place-items-center', beats > 0 && 'beat']} aria-hidden="true"
              ><span class="live-dot"></span></span
            >
          {/key}
          <span class="font-medium text-fg">Live</span>
          <span class="hidden tabular-nums md:inline">{since(conn.lastOk, motion.now)}</span>
        </span>
      {/if}
      <button
        type="button"
        class="grid size-8 place-items-center rounded-lg border border-edge text-muted transition-colors duration-150 hover:bg-hover hover:text-fg"
        aria-pressed={motion.on}
        aria-label="Motion"
        title={motion.on ? 'Motion on — click to pause animations' : 'Motion off — click to resume'}
        data-testid="motion-toggle"
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
      <div role="status" aria-live="polite">
        {#if !conn.connected}
          <span
            class="inline-flex h-8 items-center rounded-full border-[1.5px] border-warn px-3.5 text-sm font-medium text-warn"
            data-testid="disconnected"
          >
            ▲ Disconnected — last updated {hhmm(conn.lastOk)}
          </span>
        {:else if !glance}
          <span class="text-sm text-muted">Loading…</span>
        {:else if count > 0}
          {#key pulses}
            <button
              type="button"
              class={[
                'inline-flex h-8 items-center rounded-full border-[1.5px] border-bad px-3.5 text-sm font-medium',
                glance.escalated ? 'pulse-3 bg-bad text-on-accent' : 'bg-tint-bad text-bad',
              ]}
              data-testid="pill"
              onclick={() => onShowAll?.()}
            >
              {#key bumps}
                <span class={[bumps > 0 && 'bump']}
                  >■ {count} need you{glance.escalated ? ' · show all' : ''}</span
                >
              {/key}
            </button>
          {/key}
        {:else}
          <span
            class="inline-flex h-8 items-center rounded-full border-[1.5px] border-ok px-3.5 text-sm font-medium text-ok"
            data-testid="all-clear"
          >
            ● All clear
          </span>
        {/if}
      </div>
    </div>
  </div>
</header>
