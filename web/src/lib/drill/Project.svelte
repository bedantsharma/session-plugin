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
  import ConfirmRemove from '$lib/ui/ConfirmRemove.svelte'
  import OwnerButtons from '$lib/ui/OwnerButtons.svelte'
  import RowMenu from '$lib/ui/RowMenu.svelte'
  import { hillWord, statusWord } from '$lib/words'
  import { addedGroups, addedText } from '$lib/scope'
  import type { ActResult } from '$lib/api'
  import type {
    OwnerAction,
    ProjectDrill,
    ProjectRow,
    RemoveSpec,
    TrackSection,
    TreeNode,
  } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let {
    client,
    repo,
    version,
    row,
    onOpenTask,
    onOpenHistory,
    onOpenTuning,
    onOpenCalibration,
    onOpenPages,
  }: {
    client: Client
    repo: string
    version: number
    /** The glance's row for this project: done %, milestones, sessions. */
    row?: ProjectRow
    onOpenTask: (repo: string, id: string) => void
    onOpenHistory: (repo: string) => void
    onOpenTuning: (repo: string) => void
    onOpenCalibration?: (repo: string) => void
    onOpenPages?: (repo: string) => void
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
  // A line needs at least two distinct times a minute or more apart; otherwise every point
  // lands on one x and the chart collapses to a vertical line, so show the numbers instead.
  const chartable = $derived.by(() => {
    const times = (m.data?.scope ?? []).map((p) => new Date(p.ts).getTime())
    if (new Set(times).size < 2) return false
    return Math.max(...times) - Math.min(...times) >= 60_000
  })
  const act = (a: OwnerAction): Promise<ActResult> => client.act(a)
  /** One section per open track; the single-plan page is the one-track case. */
  const sections = $derived<TrackSection[]>(
    m.data?.tracks?.length
      ? m.data.tracks
      : m.data
        ? [
            {
              track: 1,
              goal: row?.goal ?? { id: 'G', shown: 'G', title: '' },
              done_pct: row?.done_pct ?? 0,
              claimed_pct: row?.claimed_pct ?? 0,
              reviewed_tasks: 0,
              unestimated: 0,
              forecast: null,
              run_number: m.data.run_number,
              tree: m.data.tree,
              completed_runs: m.data.completed_runs,
              can_close_run: m.data.can_close_run,
              finish: null,
              remove: null,
            },
          ]
        : [],
  )
  const several = $derived(sections.length > 1)
  const allNodes = $derived(sections.flatMap((t) => t.tree))
  let removing = $state<RemoveSpec | null>(null)
  let closing = $state(false)
  let closeMsg = $state<{ ok: boolean; text: string } | null>(null)
  /** "Close this run": every task passed its tests, so the next plan starts fresh. */
  async function closeRun(track: number) {
    closing = true
    closeMsg = null
    const res = await client.act({ kind: 'close_run', repo, track })
    closing = false
    closeMsg = res.ok ? { ok: true, text: res.reply } : { ok: false, text: res.error }
  }
  const latest = $derived(m.data?.scope.at(-1))
  const added = $derived(addedGroups(m.data?.scope ?? []))

  // The tree: chevrons fold a node's children away.
  let folded = $state<Record<string, boolean>>({})
  // Intents start collapsed; each has its own labelled show/hide control.
  let intentOpen = $state<Record<string, boolean>>({})
  const parents = $derived(new Set(allNodes.map((n) => n.parent_id).filter(Boolean)))
  const byId = $derived(new Map(allNodes.map((n) => [n.id, n])))
  function hidden(n: TreeNode): boolean {
    let p = n.parent_id
    while (p) {
      if (folded[p]) return true
      p = byId.get(p)?.parent_id ?? null
    }
    return false
  }

  /** The milestones of one track: the glance's, from its bar (or the project's, with one). */
  function milestonesOf(t: TrackSection) {
    return row?.tracks?.find((x) => x.track === t.track)?.milestones ?? row?.milestones ?? []
  }

  /** A node's bar: its track's numbers for the goal and milestones, else its level. */
  function progress(n: TreeNode, t: TrackSection): { done: number; claimed: number } {
    if (n.kind === 'goal') return { done: t.done_pct, claimed: t.claimed_pct }
    const ms = milestonesOf(t).find((x) => x.id === n.id)
    if (ms) return { done: ms.done_pct, claimed: ms.claimed_pct }
    if (['l2', 'l3', 'l4', 'reviewed'].includes(n.status)) return { done: 100, claimed: 0 }
    if (['claimed', 'l1'].includes(n.status)) return { done: 0, claimed: 100 }
    return { done: 0, claimed: 0 }
  }
</script>

{#snippet trackBody(t: TrackSection)}
  {#if milestonesOf(t).length}
    <section class="flex flex-col gap-3" aria-labelledby={`milestones-${t.track}`}>
      <h3 id={`milestones-${t.track}`} class="text-base font-semibold">Milestones</h3>
      <div class="flex flex-col gap-5 rounded-xl border border-line bg-surface p-5">
        <MilestoneList
          project={{ milestones: milestonesOf(t), sessions: row?.sessions ?? [] }}
          limit={100}
        />
      </div>
    </section>
  {/if}

  {#if !several}
    {#if latest}
      <section class="flex flex-col gap-3" aria-labelledby="scope">
        <h3 id="scope" class="text-base font-semibold">Scope over time</h3>
        <div class="rounded-xl border border-line bg-surface p-5">
          {#if chartable}
            <div
              class="scope-chart h-56"
              aria-label="Scope line: total live estimate and done, over time"
            >
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
          {:else}
            <p
              class="flex flex-wrap items-baseline gap-x-3 text-row tabular-nums"
              data-testid="scope-summary"
            >
              <span>Scope {latest.total_min} min · done {latest.done_min} min</span>
              <span class="text-meta text-muted"
                >a chart appears once the scope changes over time</span
              >
            </p>
          {/if}
          {#if added.length}
            <p class="mt-3 text-meta text-muted">
              Scope added: {addedText(added)}
            </p>
          {/if}
        </div>
      </section>
    {/if}
  {/if}
  <section class="flex flex-col gap-3" aria-labelledby={`plan-${t.track}`}>
    <h3 id={`plan-${t.track}`} class="text-base font-semibold">Plan</h3>
    <ul class="flex flex-col border-y border-line" aria-label="Plan tree">
      {#each t.tree as n (n.id)}
        {#if !hidden(n)}
          {@const look = statusWord(n.rollup ?? n.status)}
          {@const p = progress(n, t)}
          {@const hill =
            n.kind === 'milestone' ? hillWord(n.hill, n.rollup === 'done' ? 100 : p.done) : null}
          {@const parent = parents.has(n.id)}
          <li class="border-b border-line last:border-b-0" data-node={n.id}>
            <Collapsible.Root bind:open={() => !!intentOpen[n.id], (v) => (intentOpen[n.id] = v)}>
              <div class="flex min-h-10 flex-wrap items-center gap-2 py-1.5 sm:flex-nowrap">
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
                <span class="w-14 shrink-0 font-mono text-meta text-muted"
                  >{n.shown}{#if n.shown !== n.id}<span
                      class="ml-1 text-[0.65rem] opacity-70"
                      data-testid="raw-id"
                      title="the id agents and the command line use">({n.id})</span
                    >{/if}</span
                >
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
                  {#if n.check === 'review' && n.status !== 'reviewed'}<Chip
                      tone="muted"
                      icon="✎"
                      text="no tests"
                      tip="Checked by review: done once its result is saved"
                    />{/if}
                  {#if n.intent}
                    <Collapsible.Trigger
                      class="inline-flex h-6 items-center gap-1 rounded-md border border-edge px-2 text-meta text-muted hover:bg-hover hover:text-fg"
                      aria-label={`${intentOpen[n.id] ? 'Hide' : 'Show'} intent of ${n.title}`}
                      ><svg
                        viewBox="0 0 16 16"
                        class={[
                          'size-3 transition-transform duration-150',
                          intentOpen[n.id] && 'rotate-90',
                        ]}
                        fill="none"
                        stroke="currentColor"
                        stroke-width="2"
                        aria-hidden="true"><path d="M6 4l4 4-4 4" /></svg
                      >{intentOpen[n.id] ? 'Hide intent' : 'Show intent'}</Collapsible.Trigger
                    >
                  {/if}
                </span>
                <span class="hidden shrink-0 gap-3 text-meta text-muted tabular-nums md:flex">
                  {#if n.estimate_min !== null || n.actual_min > 0}
                    <span>{n.actual_min} of {n.estimate_min ?? '?'} min</span>
                  {/if}
                  {#if n.depends_on.length}<span>after {n.depends_on.join(', ')}</span>{/if}
                  {#each n.external_deps ?? [] as x (x.label)}<span
                      data-testid="external-dep"
                      title={x.title}>after {x.label} · {x.ready ? 'ready' : 'waiting'}</span
                    >{/each}
                  {#if n.branch}<span class="font-mono">{n.branch}</span>{/if}
                  {#if n.source_text}<span data-testid="source">{n.source_text}</span>{/if}
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
                {#if n.actions?.length}
                  <OwnerButtons actions={n.actions} {act} />
                {/if}
                {#if n.remove}
                  <RowMenu label={`Menu for step ${n.shown}`}>
                    {#snippet children(close)}
                      <button
                        type="button"
                        role="menuitem"
                        class="rounded-md px-3 py-1.5 text-left text-sm text-bad hover:bg-tint-bad"
                        onclick={() => {
                          close()
                          removing = n.remove
                        }}>Remove for good</button
                      >
                    {/snippet}
                  </RowMenu>
                {/if}
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

  {#if t.completed_runs.length}
    <section class="flex flex-col gap-3" aria-labelledby={`completed-runs-${t.track}`}>
      <h3 id={`completed-runs-${t.track}`} class="text-base font-semibold">Completed runs</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each t.completed_runs as r (r.number)}
          <li data-run={r.number}>
            <Collapsible.Root>
              <Collapsible.Trigger
                class="flex min-h-10 w-full flex-wrap items-center gap-x-4 gap-y-1 py-1.5 text-left text-row hover:bg-hover"
              >
                <span class="font-medium">Run {r.number}: {r.title}</span>
                <span class="text-meta text-muted tabular-nums"
                  >finished {r.closed_at.slice(0, 10)} · {r.tasks}
                  {r.tasks === 1 ? 'task' : 'tasks'}</span
                >
                <span class="text-meta text-ok tabular-nums">{r.done_pct}%</span>
              </Collapsible.Trigger>
              <Collapsible.Content class="pb-3">
                <ul class="flex flex-col gap-1">
                  {#each r.tree.filter((x) => x.kind === 'task' || x.kind === 'subtask') as t (t.id)}
                    <li class="flex items-center gap-2 text-row">
                      <span class="w-14 shrink-0 font-mono text-meta text-muted">{t.shown}</span>
                      <button
                        type="button"
                        class="truncate text-left hover:underline"
                        onclick={() => onOpenTask(repo, t.id)}>{t.title}</button
                      >
                    </li>
                  {/each}
                </ul>
              </Collapsible.Content>
            </Collapsible.Root>
          </li>
        {/each}
      </ul>
    </section>
  {/if}
{/snippet}

{#if m.error}
  <p class="text-row text-warn">{m.error}</p>
{:else if m.data}
  <header class="flex flex-col gap-4">
    <div class="flex flex-wrap items-start justify-between gap-4">
      <div class="flex min-w-0 flex-col gap-0.5">
        <h2 class="text-xl font-semibold">
          {m.data.name}{#if m.data.run_number > 1}<span
              class="ml-2 text-meta font-normal text-muted"
              data-testid="run-number">Run {m.data.run_number}</span
            >{/if}
        </h2>
        {#if row?.goal}<p class="text-meta text-muted">{row.goal.title}</p>{/if}
      </div>
      <div class="flex flex-wrap items-center gap-2">
        {#if m.data.can_close_run && !several}
          <button
            type="button"
            class="inline-flex h-9 items-center rounded-lg border border-ok px-4 text-sm font-medium text-ok transition-colors duration-150 hover:bg-hover disabled:opacity-60"
            disabled={closing}
            onclick={() => closeRun(sections[0]?.track ?? 1)}>Close this run</button
          >
        {/if}
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
        <button
          type="button"
          class="inline-flex h-9 items-center rounded-lg border border-edge px-4 text-sm font-medium transition-colors duration-150 hover:bg-hover"
          onclick={() => onOpenCalibration?.(repo)}>Calibration</button
        >
        <button
          type="button"
          class="inline-flex h-9 items-center rounded-lg border border-edge px-4 text-sm font-medium transition-colors duration-150 hover:bg-hover"
          onclick={() => onOpenPages?.(repo)}>Project pages</button
        >
      </div>
    </div>
    {#if closeMsg}
      <p class={['text-row', closeMsg.ok ? 'text-ok' : 'text-warn']} role="status">
        {closeMsg.text}
      </p>
    {/if}
    {#if row?.run.finished && !several}
      <p class="text-row font-medium text-ok" data-testid="run-finished">
        {row.run.finished.text}
      </p>
    {/if}
    {#if row && !several}
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

  {#if several}
    {#if latest}
      <section class="flex flex-col gap-3" aria-labelledby="scope">
        <h3 id="scope" class="text-base font-semibold">Scope over time</h3>
        <div class="rounded-xl border border-line bg-surface p-5">
          {#if chartable}
            <div
              class="scope-chart h-56"
              aria-label="Scope line: total live estimate and done, over time"
            >
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
          {:else}
            <p
              class="flex flex-wrap items-baseline gap-x-3 text-row tabular-nums"
              data-testid="scope-summary"
            >
              <span>Scope {latest.total_min} min · done {latest.done_min} min</span>
              <span class="text-meta text-muted"
                >a chart appears once the scope changes over time</span
              >
            </p>
          {/if}
          {#if added.length}
            <p class="mt-3 text-meta text-muted">
              Scope added: {addedText(added)}
            </p>
          {/if}
        </div>
      </section>
    {/if}

    {#each sections as t (t.track)}
      <section
        class="flex flex-col gap-6"
        aria-label={`Track ${t.track}: ${t.goal.title}`}
        data-track={t.track}
      >
        <div class="flex flex-col gap-3 rounded-xl border border-line bg-surface p-5">
          <div class="flex flex-wrap items-start justify-between gap-3">
            <div class="flex min-w-0 flex-col">
              <span class="text-meta text-muted">Track {t.track}</span>
              <h3 class="text-lg font-semibold">{t.goal.title}</h3>
            </div>
            <div class="flex flex-wrap items-center gap-2">
              {#if t.can_close_run}
                <button
                  type="button"
                  class="inline-flex h-9 items-center rounded-lg border border-ok px-4 text-sm font-medium text-ok transition-colors duration-150 hover:bg-hover disabled:opacity-60"
                  disabled={closing}
                  onclick={() => closeRun(t.track)}>Close this run</button
                >
              {/if}
              {#if t.finish}<OwnerButtons actions={[t.finish]} {act} compact={false} />{/if}
              {#if t.remove}
                <button
                  type="button"
                  class="inline-flex h-9 items-center rounded-lg border border-bad px-4 text-sm font-medium text-bad transition-colors duration-150 hover:bg-tint-bad"
                  onclick={() => (removing = t.remove)}>Remove for good</button
                >
              {/if}
            </div>
          </div>
          <div class="flex items-end gap-6">
            <div class="flex flex-col">
              <span class="text-[1.5rem] leading-7 font-semibold tabular-nums">{t.done_pct}%</span>
              <span class="text-meta text-muted tabular-nums"
                >done{t.claimed_pct > 0
                  ? ` · +${t.claimed_pct}% claimed`
                  : ''}{#if t.reviewed_tasks > 0}
                  · includes {t.reviewed_tasks}
                  {t.reviewed_tasks === 1 ? 'task' : 'tasks'} done without tests{/if}</span
              >
            </div>
            <div class="min-w-0 flex-1 pb-1.5">
              <ProgressBar done={t.done_pct} claimed={t.claimed_pct} />
            </div>
          </div>
          {#if t.forecast}<p class="text-meta text-muted">{t.forecast.text}</p>{/if}
        </div>
        {@render trackBody(t)}
      </section>
    {/each}
  {:else if sections[0]}
    {@render trackBody(sections[0])}
  {/if}

  {#if m.data.finished_tracks?.length}
    <section class="flex flex-col gap-3" aria-labelledby="finished-tracks">
      <h3 id="finished-tracks" class="text-base font-semibold">Finished tracks</h3>
      <ul class="flex flex-col divide-y divide-line border-y border-line">
        {#each m.data.finished_tracks as f (f.track)}
          <li
            class="flex min-h-10 flex-wrap items-center gap-3 py-1.5 text-row"
            data-finished={f.track}
          >
            <span class="font-medium">{f.goal.title}</span>
            <span class="text-meta text-muted tabular-nums"
              >track {f.track} · {f.done_pct}% done</span
            >
            <span class="ml-auto"><OwnerButtons actions={[f.reopen]} {act} /></span>
          </li>
        {/each}
      </ul>
    </section>
  {/if}

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

<ConfirmRemove spec={removing} onClose={() => (removing = null)} {act} />
