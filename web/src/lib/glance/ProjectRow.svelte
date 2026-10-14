<script lang="ts">
  // One project card (spec §10.1, §5.6; plan §2): name, goal and the big done %, a
  // captioned bar with claimed% apart, one row per milestone with its own mini bar, hill
  // and stuck chips, and the leftovers/unestimated footer. The whole card opens the
  // project drill-down.
  import ProgressBar from './ProgressBar.svelte'
  import Chip from '$lib/ui/Chip.svelte'
  import { hillWord } from '$lib/words'
  import type { ProjectRow } from '$lib/types'

  let { project, onOpen }: { project: ProjectRow; onOpen?: (repo: string) => void } = $props()

  const SHOWN = 6
  let all = $state(false)

  const complete = $derived(project.milestones.filter((m) => m.done_pct >= 100).length)
  const milestones = $derived(all ? project.milestones : project.milestones.slice(0, SHOWN))
  // Milestones a working session is on: their bar carries a slow sheen.
  const active = $derived(
    new Set(
      project.sessions
        .filter((s) => s.state_label === 'working' && s.task)
        .flatMap((s) =>
          project.milestones
            .filter((m) => s.task!.id === m.id || s.task!.id.startsWith(`${m.id}.`))
            .map((m) => m.id),
        ),
    ),
  )
</script>

<li
  class="relative flex flex-col gap-5 rounded-xl border border-line bg-surface p-5 transition-colors duration-150 hover:border-edge"
  data-project={project.repo_id}
>
  <div class="flex items-start justify-between gap-4">
    <div class="flex min-w-0 flex-col">
      <button
        type="button"
        class="self-start text-left text-base font-semibold outline-none after:absolute after:inset-0 after:rounded-xl hover:underline focus-visible:after:ring-3 focus-visible:after:ring-ring/50"
        onclick={() => onOpen?.(project.repo_id)}>{project.name}</button
      >
      {#if project.goal}
        <span class="truncate text-meta text-muted">{project.goal.title}</span>
      {/if}
    </div>
    <div class="flex shrink-0 flex-col items-end" data-testid="progress-text">
      <span class="text-[1.75rem] leading-8 font-semibold tabular-nums">
        {#key project.done_pct}<span class="num">{project.done_pct}%</span>{/key}
      </span>
      {#if project.claimed_pct > 0}
        <span class="text-meta text-muted tabular-nums">+{project.claimed_pct}% claimed</span>
      {/if}
    </div>
  </div>

  <div class="flex flex-col gap-2">
    <p class="text-meta text-muted tabular-nums" data-testid="progress-caption">
      Done {project.done_pct}% · {complete} of {project.milestones.length}
      {project.milestones.length === 1 ? 'milestone' : 'milestones'} complete
    </p>
    <ProgressBar done={project.done_pct} claimed={project.claimed_pct} />
  </div>

  {#if project.milestones.length}
    <ul class="-mt-2 grid grid-cols-[2.5rem_minmax(0,1fr)_7rem_3rem_auto] gap-x-3">
      {#each milestones as m (m.id)}
        {@const hill = hillWord(m.hill, m.done_pct)}
        <li class="col-span-5 grid h-9 grid-cols-subgrid items-center" data-milestone={m.id}>
          <span class="font-mono text-meta text-muted">{m.id}</span>
          <span class="truncate text-row">{m.title}</span>
          <ProgressBar
            done={m.done_pct}
            claimed={m.claimed_pct}
            size="mini"
            labelled={false}
            active={active.has(m.id)}
          />
          <span
            class={['text-right text-meta tabular-nums', m.hill === 'uphill' && 'text-muted']}
            data-testid={m.hill === 'uphill' ? 'exploring-pct' : undefined}>{m.done_pct}%</span
          >
          <span class="flex items-center gap-1.5">
            {#if hill}<Chip
                tone={hill.tone}
                icon={hill.icon}
                text={hill.text}
                tip={hill.tip}
              />{/if}
            {#if m.stuck}
              <Chip tone="warn" icon="!" text={`stuck · ${m.stuck_sessions} sessions`} />
            {/if}
          </span>
        </li>
      {/each}
    </ul>
    {#if project.milestones.length > SHOWN}
      <button
        type="button"
        class="relative z-10 -mt-3 self-start rounded-lg px-2 py-1 text-sm font-medium text-muted hover:bg-hover hover:text-fg"
        onclick={() => (all = !all)}
        >{all ? `show the first ${SHOWN}` : `show all ${project.milestones.length}`}</button
      >
    {/if}
  {/if}

  {#if project.leftovers.open > 0 || project.unestimated > 0}
    <div class="flex flex-wrap gap-x-4 border-t border-line pt-3 text-meta text-muted">
      {#if project.leftovers.open > 0}
        <span
          >{project.leftovers.open}
          {project.leftovers.open === 1 ? 'leftover' : 'leftovers'}</span
        >
      {/if}
      {#if project.unestimated > 0}<span>{project.unestimated} unestimated</span>{/if}
    </div>
  {/if}
</li>
