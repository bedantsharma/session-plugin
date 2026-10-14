<script lang="ts">
  // The glance header (spec §10.1): ✱, counts, and the one number that matters — the
  // red "N need you" pill, or green "All clear". Never green on stale data: when the
  // server is gone or the token refused it says Disconnected instead.
  import type { ConnState } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { Glance } from '$lib/types'

  let {
    glance,
    conn,
    onShowAll,
  }: { glance: Glance | null; conn: ConnState; onShowAll?: () => void } = $props()

  const count = $derived(glance?.needs_you_count ?? 0)
  // Pulse each time the count rises past 4 (at most 3 cycles, under 5 s; a fade under
  // reduced motion), then stay still and emphasised.
  let pulses = $state(0)
  let previous = 0
  $effect(() => {
    if (count > 4 && previous <= 4) pulses += 1
    previous = count
  })
</script>

<header class="flex items-center justify-between gap-4 border-b border-line pb-3">
  <h1 class="flex items-baseline gap-2 text-base font-medium">
    <span class="text-clay" aria-hidden="true">✱</span>
    <span>bridge-screen</span>
    {#if glance}
      <span class="text-sm font-normal text-muted">
        {glance.live_sessions}
        {glance.live_sessions === 1 ? 'session' : 'sessions'} ·
        {glance.projects.length}
        {glance.projects.length === 1 ? 'project' : 'projects'}
      </span>
    {/if}
  </h1>
  <div role="status" aria-live="polite">
    {#if !conn.connected}
      <span
        class="rounded-full border border-warn px-3 py-1 text-sm text-warn"
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
            'rounded-full border-2 border-bad px-3 py-1 text-sm font-medium',
            glance.escalated ? 'pulse-3 bg-bad text-on-accent' : 'text-bad',
          ]}
          data-testid="pill"
          onclick={() => onShowAll?.()}
        >
          ■ {count} need you{glance.escalated ? ' · show all' : ''}
        </button>
      {/key}
    {:else}
      <span class="rounded-full border border-ok px-3 py-1 text-sm text-ok" data-testid="all-clear">
        ● All clear
      </span>
    {/if}
  </div>
</header>
