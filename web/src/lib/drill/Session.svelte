<script lang="ts">
  // Session drill-down (spec §10.2, plan §2): the header block, time by label as a
  // captioned bar with a legend, the timeline of units as a fixed-column table with
  // readable labels, then suspended items, checkpoints, leftovers, the handoff note, the
  // read-back and the owner's answers.
  import Chip from '$lib/ui/Chip.svelte'
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import {
    answerWord,
    filesWord,
    leftoverWord,
    mins,
    readbackWord,
    unitWord,
    type ChipTone,
  } from '$lib/words'
  import type { SessionDrill } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let { client, id, version }: { client: Client; id: string; version: number } = $props()

  const m = useModel<SessionDrill>(
    () => client,
    () => `/api/session/${encodeURIComponent(id)}`,
    () => version,
  )

  const detourS = $derived(m.data ? m.data.split.detours.reduce((a, d) => a + d.secs, 0) : 0)
  const total = $derived(
    m.data
      ? m.data.split.on_task_s + m.data.split.drift_s + m.data.split.unlabeled_s + detourS || 1
      : 1,
  )
  const active = $derived(m.data ? m.data.units.reduce((a, u) => a + u.active_s, 0) : 0)
  const pct = (s: number) => `${(s / total) * 100}%`

  const states: Record<string, { icon: string; text: string; tone: ChipTone }> = {
    working: { icon: '●', text: 'Working', tone: 'ok' },
    idle: { icon: '◆', text: 'Idle', tone: 'warn' },
    needs_owner: { icon: '■', text: 'Needs you', tone: 'bad' },
    ended: { icon: '○', text: 'Ended', tone: 'muted' },
  }
  // Fixed columns from 1024 px; below, a row wraps to a primary line and a metadata line.
  const COLS = 'lg:grid lg:grid-cols-[3rem_4.5rem_4.5rem_4rem_11rem_4rem_minmax(0,1fr)]'
</script>

{#if m.error}
  <p class="text-row text-warn">{m.error}</p>
{:else if m.data}
  {@const d = m.data}
  {@const st = states[d.state] ?? {
    icon: '·',
    text: d.state.replaceAll('_', ' '),
    tone: 'muted' as ChipTone,
  }}
  <header class="flex flex-col gap-2">
    <div class="flex flex-wrap items-center gap-3">
      <h2 class="font-mono text-xl font-semibold">S {d.short}</h2>
      <Chip tone={st.tone} icon={st.icon} text={st.text} live={d.state === 'working'} />
      <span class="ml-auto text-row tabular-nums">{mins(active)} active</span>
    </div>
    {#if d.cost}
      <p class="text-meta tabular-nums" data-testid="session-cost">{d.cost.summary}</p>
    {/if}
    <div class="flex flex-wrap gap-x-4 gap-y-1 text-meta text-muted">
      {#if d.task}<span>{d.task.id} {d.task.title}</span>{/if}
      {#if d.branch}<span class="font-mono">{d.branch}</span>{/if}
      {#if d.worktree}<span class="truncate font-mono">{d.worktree}</span>{/if}
    </div>
  </header>

  <section class="flex flex-col gap-3" aria-labelledby="time-by-label">
    <h3 id="time-by-label" class="text-base font-semibold">Time by label</h3>
    <div class="flex flex-col gap-3 rounded-xl border border-line bg-surface p-5">
      <p class="text-meta text-muted tabular-nums">
        {mins(total)} in {d.units.length}
        {d.units.length === 1 ? 'unit' : 'units'}
      </p>
      <div
        class="flex h-2.5 w-full overflow-hidden rounded-full bg-line"
        role="img"
        aria-label={`On task ${mins(d.split.on_task_s)}, detour ${mins(detourS)}, drift ${mins(d.split.drift_s)}, unlabelled ${mins(d.split.unlabeled_s)}`}
      >
        <div class="bar-fill h-full bg-ok" style:width={pct(d.split.on_task_s)}></div>
        <div class="bar-fill h-full bg-warn" style:width={pct(detourS)}></div>
        <div class="bar-fill h-full bg-bad" style:width={pct(d.split.drift_s)}></div>
      </div>
      <ul class="flex flex-wrap gap-x-6 gap-y-1 text-meta tabular-nums">
        <li class="flex items-center gap-2">
          <span class="size-3 rounded-full bg-ok"></span>On task {mins(d.split.on_task_s)}
        </li>
        <li class="flex items-center gap-2">
          <span class="size-3 rounded-full bg-warn"></span>Detour {mins(detourS)}
          {#each d.split.detours as dt (dt.label)}
            <span class="text-muted">· {dt.label} {dt.reason} {mins(dt.secs)}</span>
          {/each}
        </li>
        <li class="flex items-center gap-2">
          <span class="size-3 rounded-full bg-bad"></span>Drift {mins(d.split.drift_s)}
        </li>
        <li class="flex items-center gap-2 text-muted">
          <span class="size-3 rounded-full border border-edge bg-line"></span>Unlabelled
          {mins(d.split.unlabeled_s)}
        </li>
      </ul>
    </div>
  </section>

  <section class="flex flex-col gap-3" aria-labelledby="timeline">
    <h3 id="timeline" class="text-base font-semibold">Timeline</h3>
    <div class="rounded-xl border border-line bg-surface" role="table" aria-label="Units">
      <div
        class={[
          'sticky top-0 z-10 hidden h-9 items-center gap-3 rounded-t-xl border-b border-line bg-surface px-5 text-chip font-medium tracking-wide text-muted uppercase',
          COLS,
        ]}
        role="row"
      >
        <span role="columnheader">#</span>
        <span role="columnheader">Time</span>
        <span role="columnheader" class="text-right">Length</span>
        <span role="columnheader" class="text-right">Tokens</span>
        <span role="columnheader">Label</span>
        <span role="columnheader">Task</span>
        <span role="columnheader">Files · score</span>
      </div>
      {#each d.units as u (u.unit_no)}
        {@const w = unitWord(u.label, u.label_source)}
        <div
          class={[
            'flex flex-wrap items-center gap-x-3 gap-y-1 border-b border-line px-5 py-2 text-meta last:border-b-0 lg:h-9 lg:py-0',
            COLS,
          ]}
          role="row"
          data-unit={u.unit_no}
        >
          <span role="cell" class="font-mono text-muted">#{u.unit_no}</span>
          <span role="cell" class="text-muted tabular-nums">{hhmm(u.started_at)}</span>
          <span role="cell" class="text-right tabular-nums">{mins(u.active_s)}</span>
          <span role="cell" class="text-right text-muted tabular-nums" data-testid="unit-tokens"
            >{u.tokens_text ?? '—'}</span
          >
          <span role="cell" class="whitespace-nowrap"
            ><Chip tone={w.tone} icon={w.icon} text={w.text} /></span
          >
          <span role="cell" class="truncate font-mono">{u.task_id ?? '—'}</span>
          <span role="cell" class="truncate text-muted tabular-nums">
            {#if u.detour}<span class="text-warn">{u.detour}</span> ·{/if}
            {filesWord(u.files)}{u.drift_score !== null
              ? ` · score ${u.drift_score.toFixed(2)}`
              : ''}
            {#if u.disagreement}
              <span class="text-warn" title="The agent's label and the drift score disagree"
                >· ⚠ disagree</span
              >
            {/if}
          </span>
        </div>
      {:else}
        <p class="px-5 py-3 text-row text-muted">No finished units yet.</p>
      {/each}
    </div>
  </section>

  {#if d.owner_answers.length}
    <section class="flex flex-col gap-3">
      <h3 class="text-base font-semibold">Your answers</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each d.owner_answers as a (a.ts)}
          <li class="flex min-h-11 items-center gap-3 text-row">
            <span class="w-14 shrink-0 text-meta text-muted tabular-nums">{hhmm(a.ts)}</span>
            <span class="text-clay">{answerWord(a.answer)}</span>
            {#if a.reason}<span class="text-muted">“{a.reason}”</span>{/if}
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if d.suspended.length}
    <section class="flex flex-col gap-3">
      <h3 class="text-base font-semibold">Suspended</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each d.suspended as s (s.item_id)}
          <li class="flex min-h-11 flex-col justify-center gap-0.5 py-2">
            <span class="text-row">{s.what}</span>
            <span class="text-meta text-muted">On return: {s.resume_cue}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if d.checkpoints.length}
    <section class="flex flex-col gap-3">
      <h3 class="text-base font-semibold">Checkpoints</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each d.checkpoints as c (c.sha)}
          <li class="flex min-h-11 items-center gap-3 py-2 text-row">
            <span class="font-mono text-meta text-muted">{c.sha.slice(0, 8)}</span>
            {#if c.task_id}<span class="font-mono text-meta">{c.task_id}</span>{/if}
            <span class="min-w-0 truncate">Next: {c.next_action ?? '—'}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if d.leftovers.length}
    <section class="flex flex-col gap-3">
      <h3 class="text-base font-semibold">Leftovers</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each d.leftovers as l (l.leftover_id)}
          <li class="flex min-h-11 items-center gap-3 py-2 text-row">
            <span class="min-w-0 truncate font-mono text-meta">{l.path}</span>
            <span class="ml-auto shrink-0 text-meta text-muted">{leftoverWord(l)}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

  {#if d.handoff_note}
    <section class="flex flex-col gap-3">
      <h3 class="text-base font-semibold">Handoff note</h3>
      <div class="flex max-w-[80ch] flex-col gap-2 rounded-xl border border-line bg-surface p-5">
        <p class="text-row">{d.handoff_note.in_flight}</p>
        {#if d.handoff_note.if_then.length}
          <ul class="flex list-disc flex-col gap-1 pl-5 text-meta text-muted">
            {#each d.handoff_note.if_then as w (w)}<li>{w}</li>{/each}
          </ul>
        {/if}
      </div>
    </section>
  {/if}

  <p class="text-meta text-muted" data-testid="readback">Read-back: {readbackWord(d.readback)}</p>
{:else}
  <p class="text-row text-muted">Loading…</p>
{/if}
