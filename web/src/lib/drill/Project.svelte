<script lang="ts">
  // Project drill-down (spec §10.2, plan §2): the name, goal and big done %; the
  // milestone card; the burn-up of the scope line (added scope shows as steps up the
  // total line, listed with their times); the plan tree with intents collapsed, levels and
  // stale badges, estimate vs actual and dependencies; and re-plan history.
  import { LineChart } from 'layerchart'
  import * as Collapsible from '$lib/components/ui/collapsible'
  import MilestoneList from '$lib/glance/MilestoneList.svelte'
  import ProgressBar from '$lib/glance/ProgressBar.svelte'
  import Chip from '$lib/ui/Chip.svelte'
  import type { Client } from '$lib/api'
  import { changed } from '$lib/changed'
  import { hhmm } from '$lib/format'
  import { hillWord } from '$lib/words'
  import type { ProjectDrill, ProjectRow, TreeNode } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let {
    client,
    repo,
    version,
    row,
    onOpenTask,
    onOpenHistory,
    onOpenTuning,
  }: {
    client: Client
    repo: string
    version: number
    /** The glance's row for this project: done %, milestones, sessions. */
    row?: ProjectRow
    onOpenTask: (repo: string, id: string) => void
    onOpenHistory: (repo: string) => void
    onOpenTuning: (repo: string) => void
  } = $props()

  const m = useModel<ProjectDrill>(
    () => client,
    () => `/api/project/${encodeURIComponent(repo)}`,
    () => version,
  )

  // Steps: hold each value until the next change.
  const points = $derived.by(() => {
    const out: { date: Date; total: number; done: number }[] = []
    for (const p of m.data?.scope ?? []) {
      const date = new Date(p.ts)
      const last = out.at(-1)
      if (last) out.push({ date, total: last.total, done: last.done })
      out.push({ date, total: p.total_min, done: p.done_min })
    }
    return out
  })
  const added = $derived(
    (m.data?.scope ?? []).flatMap((p, i, all) =>
      i > 0 && p.total_min > all[i - 1].total_min
        ? [{ ts: p.ts, by: p.total_min - all[i - 1].total_min }]
        : [],
    ),
  )

  // The tree: chevrons fold a node's children away.
  let folded = $state<Record<string, boolean>>({})
  const parents = $derived(new Set((m.data?.tree ?? []).map((n) => n.parent_id).filter(Boolean)))
  const byId = $derived(new Map((m.data?.tree ?? []).map((n) => [n.id, n])))
  function hidden(n: TreeNode): boolean {
    let p = n.parent_id
    while (p) {
      if (folded[p]) return true
      p = byId.get(p)?.parent_id ?? null
    }
    return false
  }

  /** A node's bar: the glance's numbers for the goal and milestones, else its level. */
  function progress(n: TreeNode): { done: number; claimed: number } {
    if (n.kind === 'goal' && row) return { done: row.done_pct, claimed: row.claimed_pct }
    const ms = row?.milestones.find((x) => x.id === n.id)
    if (ms) return { done: ms.done_pct, claimed: ms.claimed_pct }
    if (['l2', 'l3', 'l4'].includes(n.status)) return { done: 100, claimed: 0 }
    if (['claimed', 'l1'].includes(n.status)) return { done: 0, claimed: 100 }
    return { done: 0, claimed: 0 }
  }

  const statusLook: Record<string, { icon: string; word: string; cls: string }> = {
    todo: { icon: '○', word: 'to do', cls: 'text-muted' },
    active: { icon: '◔', word: 'active', cls: 'text-clay' },
    claimed: { icon: '◑', word: 'claimed', cls: 'text-clay' },
    l1: { icon: '◕', word: 'L1 code exists', cls: 'text-clay' },
    l2: { icon: '✓', word: 'L2 tests pass', cls: 'text-ok' },
    l3: { icon: '✓', word: 'L3 checked', cls: 'text-ok' },
    l4: { icon: '✓', word: 'L4 owner-checked', cls: 'text-ok' },
    failed: { icon: '✗', word: 'failed', cls: 'text-bad' },
    dropped: { icon: '–', word: 'dropped', cls: 'text-muted' },
  }
</script>

{#if m.error}
  <p class="text-row text-warn">{m.error}</p>
{:else if m.data}
  <header class="flex flex-col gap-4">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div class="flex min-w-0 flex-col gap-0.5">
        <h2 class="text-xl font-semibold">{m.data.name}</h2>
        {#if row?.goal}<p class="text-meta text-muted">{row.goal.title}</p>{/if}
      </div>
      <div class="flex items-center gap-2">
        <button
          type="button"
          class="inline-flex h-9 items-center rounded-lg border border-edge px-4 text-sm font-medium transition-colors duration-150 hover:bg-hover"
          onclick={() => onOpenHistory(repo)}>History</button
        >
        <button
          type="button"
          class="inline-flex h-9 items-center rounded-lg border border-edge px-4 text-sm font-medium transition-colors duration-150 hover:bg-hover"
          onclick={() => onOpenTuning(repo)}>Drift tuning</button
        >
      </div>
    </div>
    {#if row}
      <div class="flex items-end gap-6">
        <div class="flex flex-col">
          <span class="text-[1.75rem] leading-8 font-semibold tabular-nums">
            <span class="inline-block" use:changed={{ value: row.done_pct, cls: 'num' }}
              >{row.done_pct}%</span
            >
          </span>
          <span class="text-meta text-muted tabular-nums"
            >done{row.claimed_pct > 0 ? ` · +${row.claimed_pct}% claimed` : ''}</span
          >
        </div>
        <div class="min-w-0 flex-1 pb-1.5">
          <ProgressBar done={row.done_pct} claimed={row.claimed_pct} />
        </div>
      </div>
    {/if}
  </header>

  {#if row?.milestones.length}
    <section class="flex flex-col gap-3" aria-labelledby="milestones">
      <h3 id="milestones" class="text-base font-semibold">Milestones</h3>
      <div class="flex flex-col gap-5 rounded-xl border border-line bg-surface p-5">
        <MilestoneList project={row} limit={100} />
      </div>
    </section>
  {/if}

  {#if points.length > 1}
    <section class="flex flex-col gap-3" aria-labelledby="scope">
      <h3 id="scope" class="text-base font-semibold">Scope over time</h3>
      <div class="rounded-xl border border-line bg-surface p-5">
        <div class="h-56" aria-label="Scope line: total live estimate and done, over time">
          <LineChart
            data={points}
            x="date"
            series={[
              { key: 'total', label: 'scope (min)', color: 'var(--muted)' },
              { key: 'done', label: 'done (min)', color: 'var(--clay)' },
            ]}
            legend
          />
        </div>
        {#if added.length}
          <p class="mt-3 text-meta text-muted">
            Scope added: {added.map((a) => `+${a.by} min at ${hhmm(a.ts)}`).join(' · ')}
          </p>
        {/if}
      </div>
    </section>
  {/if}

  <section class="flex flex-col gap-3" aria-labelledby="plan">
    <h3 id="plan" class="text-base font-semibold">Plan</h3>
    <ul class="flex flex-col border-y border-line" aria-label="Plan tree">
      {#each m.data.tree as n (n.id)}
        {#if !hidden(n)}
          {@const look = statusLook[n.status] ?? { icon: '·', word: n.status, cls: 'text-muted' }}
          {@const p = progress(n)}
          {@const hill = n.kind === 'milestone' ? hillWord(n.hill, p.done) : null}
          {@const parent = parents.has(n.id)}
          <li class="border-b border-line last:border-b-0" data-node={n.id}>
            <Collapsible.Root>
              <div class="flex min-h-10 items-center gap-2 py-1.5">
                <!-- Indent guides: one 20 px step per level. -->
                {#each Array.from({ length: n.depth }, (_, i) => i) as i (i)}
                  <span
                    class="-my-1.5 w-5 shrink-0 self-stretch border-l border-line"
                    aria-hidden="true"
                  ></span>
                {/each}
                {#if parent}
                  <button
                    type="button"
                    class="grid size-6 shrink-0 place-items-center rounded-md text-muted hover:bg-hover hover:text-fg"
                    aria-expanded={!folded[n.id]}
                    aria-label={`${folded[n.id] ? 'Show' : 'Hide'} the steps under ${n.title}`}
                    onclick={() => (folded[n.id] = !folded[n.id])}
                  >
                    <svg
                      viewBox="0 0 16 16"
                      class={[
                        'size-4 transition-transform duration-150',
                        !folded[n.id] && 'rotate-90',
                      ]}
                      fill="none"
                      stroke="currentColor"
                      stroke-width="1.75"
                      aria-hidden="true"><path d="M6 4l4 4-4 4" /></svg
                    >
                  </button>
                {:else}
                  <span class="size-6 shrink-0" aria-hidden="true"></span>
                {/if}
                <span
                  class={['w-4 shrink-0 text-center', look.cls]}
                  title={look.word}
                  aria-hidden="true">{look.icon}</span
                >
                <span class="w-10 shrink-0 font-mono text-meta text-muted">{n.id}</span>
                <span class="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1">
                  {#if n.kind === 'task' || n.kind === 'subtask'}
                    <button
                      type="button"
                      class={[
                        'truncate text-left text-row hover:underline',
                        parent && 'font-medium',
                      ]}
                      onclick={() => onOpenTask(repo, n.id)}>{n.title}</button
                    >
                  {:else}
                    <span class={['truncate text-row', parent && 'font-medium']}>{n.title}</span>
                  {/if}
                  {#if hill}<Chip
                      tone={hill.tone}
                      icon={hill.icon}
                      text={hill.text}
                      tip={hill.tip}
                    />{/if}
                  {#if n.stale}<Chip tone="warn" icon="!" text="stale" />{/if}
                  {#if n.owner_checked}<Chip tone="clay" icon="✓" text="owner" />{/if}
                  {#if n.intent}
                    <Collapsible.Trigger
                      class="rounded-md px-1.5 text-meta text-muted hover:bg-hover hover:text-fg"
                      >intent</Collapsible.Trigger
                    >
                  {/if}
                </span>
                <span class="hidden shrink-0 gap-3 text-meta text-muted tabular-nums md:flex">
                  {#if n.estimate_min !== null || n.actual_min > 0}
                    <span>{n.actual_min} of {n.estimate_min ?? '?'} min</span>
                  {/if}
                  {#if n.depends_on.length}<span>after {n.depends_on.join(', ')}</span>{/if}
                  {#if n.decisions}<span
                      >{n.decisions} {n.decisions === 1 ? 'decision' : 'decisions'}</span
                    >{/if}
                </span>
                <span class="w-24 shrink-0"
                  ><ProgressBar
                    done={p.done}
                    claimed={p.claimed}
                    size="mini"
                    labelled={false}
                  /></span
                >
                <span class="w-10 shrink-0 text-right text-meta tabular-nums">{p.done}%</span>
                <span class="sr-only shrink-0 text-meta text-muted lg:not-sr-only lg:w-28"
                  >{look.word}</span
                >
              </div>
              {#if n.intent}
                <Collapsible.Content
                  class="pb-3 text-meta text-muted"
                  style={`padding-left: ${n.depth * 1.25 + 4.5}rem`}>{n.intent}</Collapsible.Content
                >
              {/if}
            </Collapsible.Root>
          </li>
        {/if}
      {/each}
    </ul>
  </section>

  {#if m.data.replans.length}
    <section class="flex flex-col gap-3">
      <h3 class="text-base font-semibold">Re-plans</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each m.data.replans as r (r.ts)}
          <li class="flex min-h-9 items-center gap-3 text-row">
            <span class="w-14 text-meta text-muted tabular-nums">{hhmm(r.ts)}</span>
            <span class="text-meta">{r.op_count} {r.op_count === 1 ? 'change' : 'changes'}</span>
            <span class="text-muted">{r.reason ?? ''}</span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
{:else}
  <p class="text-row text-muted">Loading…</p>
{/if}
