<script lang="ts">
  // Session drill-down (spec §10.2): labelled units, the time split, suspended items,
  // checkpoints, leftovers, the handoff note, the read-back and the owner's answers.
  import type { Client } from '$lib/api'
  import { hhmm, minutes } from '$lib/format'
  import type { SessionDrill } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let { client, id, version }: { client: Client; id: string; version: number } = $props()

  const m = useModel<SessionDrill>(
    () => client,
    () => `/api/session/${encodeURIComponent(id)}`,
    () => version,
  )
  const total = $derived(
    m.data
      ? m.data.split.on_task_s +
          m.data.split.drift_s +
          m.data.split.unlabeled_s +
          m.data.split.detours.reduce((a, d) => a + d.secs, 0) || 1
      : 1,
  )
  const labelColour: Record<string, string> = {
    on_task: 'text-ok',
    detour: 'text-warn',
    drift: 'text-bad',
  }
</script>

{#if m.error}
  <p class="text-warn">{m.error}</p>
{:else if m.data}
  {@const d = m.data}
  <h2 class="text-lg font-medium">
    <span class="font-mono">S {d.short}</span>
    <span class="text-sm text-muted"
      >{d.state} · {d.branch ?? ''} · <span class="font-mono">{d.worktree ?? ''}</span></span
    >
  </h2>
  <div
    class="mt-2 flex h-3 w-full overflow-hidden rounded border border-edge"
    role="img"
    aria-label="Time split"
  >
    <div class="bg-ok" style:width={`${(d.split.on_task_s / total) * 100}%`}></div>
    {#each d.split.detours as dt (dt.label)}
      <div class="bg-warn" style:width={`${(dt.secs / total) * 100}%`}></div>
    {/each}
    <div class="bg-bad" style:width={`${(d.split.drift_s / total) * 100}%`}></div>
  </div>
  <p class="mt-1 text-xs text-muted">
    on task {minutes(d.split.on_task_s)}
    {#each d.split.detours as dt (dt.label)}
      · {dt.label} {dt.reason} {minutes(dt.secs)}{/each}
    · drift {minutes(d.split.drift_s)} · unlabelled {minutes(d.split.unlabeled_s)}
  </p>
  <ol class="mt-3 text-sm">
    {#each d.units as u (u.unit_no)}
      <li class="flex gap-3">
        <span class="w-8 text-muted">#{u.unit_no}</span>
        <span class="w-12 text-muted">{hhmm(u.started_at)}</span>
        <span class="w-12">{minutes(u.active_s)}</span>
        <span class={['w-28', labelColour[u.label ?? ''] ?? 'text-muted']}
          >{u.label ?? 'unlabelled'}{u.label_source ? ` (${u.label_source})` : ''}{u.disagreement
            ? ' ⚠'
            : ''}</span
        >
        <span class="text-muted"
          >{u.detour ?? ''}
          {u.task_id ?? ''} · {u.files} files
          {u.drift_score !== null ? `· score ${u.drift_score.toFixed(2)}` : ''}</span
        >
      </li>
    {/each}
    {#each d.owner_answers as a (a.ts)}
      <li class="text-xs text-clay">
        {hhmm(a.ts)} owner: {a.answer}{a.reason ? ` — “${a.reason}”` : ''}
      </li>
    {/each}
  </ol>
  {#if d.suspended.length}
    <h3 class="mt-3 text-sm font-medium">Suspended</h3>
    <ul class="text-xs">
      {#each d.suspended as s (s.item_id)}<li>{s.what} — on return: {s.resume_cue}</li>{/each}
    </ul>
  {/if}
  {#if d.checkpoints.length}
    <h3 class="mt-3 text-sm font-medium">Checkpoints</h3>
    <ul class="text-xs">
      {#each d.checkpoints as c (c.sha)}<li>
          <span class="font-mono">{c.sha.slice(0, 8)}</span>
          {c.task_id ?? ''} — next: {c.next_action ?? ''}
        </li>{/each}
    </ul>
  {/if}
  {#if d.leftovers.length}
    <h3 class="mt-3 text-sm font-medium">Leftovers</h3>
    <ul class="text-xs">
      {#each d.leftovers as l (l.leftover_id)}<li>
          <span class="font-mono">{l.path}</span> → {l.resolution ??
            `${l.state} (suggested ${l.suggested})`}
        </li>{/each}
    </ul>
  {/if}
  {#if d.handoff_note}
    <h3 class="mt-3 text-sm font-medium">Handoff note</h3>
    <p class="text-xs">{d.handoff_note.in_flight}</p>
    <ul class="text-xs text-muted">
      {#each d.handoff_note.if_then as w (w)}<li>{w}</li>{/each}
    </ul>
  {/if}
  <p class="mt-3 text-xs text-muted">read-back: {d.readback}</p>
{:else}
  <p class="text-muted">Loading…</p>
{/if}
