<script lang="ts">
  // One live session (plan §2): a status chip with its word and a distinct shape per
  // state (colour is never the only signal), the mono id, its task or detour, the branch
  // and the active time. A working session gets a green bar, a faint tint and a ring
  // that breathes around its dot (still under Motion off or reduced motion).
  import Chip from '$lib/ui/Chip.svelte'
  import { flagText } from '$lib/format'
  import { sessionWord } from '$lib/words'
  import type { SessionRow } from '$lib/types'

  let { session, onOpen }: { session: SessionRow; onOpen?: (id: string) => void } = $props()

  const word = $derived(sessionWord(session))
  const working = $derived(session.state_label === 'working')
</script>

<li
  class={[
    'relative flex min-h-[52px] items-center gap-3 rounded-lg border-l-2 py-2 pr-3 pl-3 transition-colors duration-150',
    working ? 'border-l-ok bg-tint-working' : 'border-l-transparent hover:bg-hover',
  ]}
  data-session={session.session_id}
  data-working={working || undefined}
>
  <div class="flex min-w-0 flex-1 flex-col gap-0.5">
    <div class="flex min-w-0 items-center gap-3">
      <Chip tone={word.tone} icon={word.icon} text={word.text} live={working} />
      <button
        type="button"
        class="shrink-0 font-mono text-meta outline-none after:absolute after:inset-0 after:rounded-lg hover:underline focus-visible:after:ring-3 focus-visible:after:ring-ring/50"
        onclick={() => onOpen?.(session.session_id)}>S {session.short}</button
      >
      <span class="min-w-0 flex-1 truncate text-row">
        {#if session.detour}
          <span class="text-warn">{session.detour.label}</span>
          {session.detour.reason}
          {#if session.task}<span class="text-muted"> · for {session.task.id}</span>{/if}
        {:else if session.task}
          {session.task.id} {session.task.title}
        {:else}
          <span class="text-muted">(no task)</span>
        {/if}
      </span>
    </div>
    {#if session.branch || session.flags.length}
      <div class="flex min-w-0 items-center gap-2 text-meta text-muted">
        {#if session.branch}<span class="truncate font-mono">{session.branch}</span>{/if}
        {#each session.flags as f (f.kind)}
          <span class="shrink-0">· {flagText(f)}</span>
        {/each}
      </div>
    {/if}
  </div>
  <span class="shrink-0 self-start pt-0.5 text-meta text-muted tabular-nums"
    >{#key session.active_min}<span class="num">{session.active_min} min</span>{/key}</span
  >
</li>
