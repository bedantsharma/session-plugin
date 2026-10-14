<script lang="ts">
  // One live session: a light with a word and a distinct shape per state (colour is never
  // the only signal), its task or detour, flags with ages, monospace ids.
  import { flagText } from '$lib/format'
  import type { SessionRow } from '$lib/types'

  let { session, onOpen }: { session: SessionRow; onOpen?: (id: string) => void } = $props()

  const shape: Record<string, string> = { green: '●', amber: '◆', red: '■' }
  const colour: Record<string, string> = { green: 'text-ok', amber: 'text-warn', red: 'text-bad' }
</script>

<li class="flex items-center gap-3 px-2 py-1 text-sm" data-session={session.session_id}>
  <span class={['w-24 shrink-0 text-meta font-medium', colour[session.light]]}>
    <span aria-hidden="true">{shape[session.light]}</span>
    {session.light_word}
  </span>
  <button
    type="button"
    class="shrink-0 font-mono text-meta hover:underline"
    onclick={() => onOpen?.(session.session_id)}>S {session.short}</button
  >
  <span class="min-w-0 flex-1 truncate">
    {#if session.detour}
      <span class="text-warn">{session.detour.label}</span>
      {session.detour.reason}
      {#if session.task}<span class="text-muted"> · for {session.task.id}</span>{/if}
    {:else if session.task}
      {session.task.id} {session.task.title}
    {:else}
      <span class="text-muted">(no task)</span>
    {/if}
    {#each session.flags as f (f.kind)}
      <span class="ml-2 text-meta text-muted">· {flagText(f)}</span>
    {/each}
  </span>
  {#if session.branch}<span class="hidden shrink-0 font-mono text-meta text-muted lg:inline"
      >{session.branch}</span
    >{/if}
  <span class="shrink-0 text-meta text-muted">{session.active_min}m</span>
</li>
