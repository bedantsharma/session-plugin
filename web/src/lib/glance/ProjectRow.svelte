<script lang="ts">
  // One project card (spec §10.1, §5.6; plan §2): name, goal and the big done %, the
  // forecast range under it (or the reason there is none), a captioned bar with claimed%
  // apart, one row per milestone with its own mini bar, hill and stuck chips, and the
  // cost/leftovers/unestimated footer. The whole card opens the project drill-down.
  import MilestoneList from './MilestoneList.svelte'
  import { changed } from '$lib/changed'
  import ProgressBar from './ProgressBar.svelte'
  import type { ProjectRow } from '$lib/types'
  import Chip from '$lib/ui/Chip.svelte'
  import { rollupWord } from '$lib/words'

  let { project, onOpen }: { project: ProjectRow; onOpen?: (repo: string) => void } = $props()

  const complete = $derived(
    project.milestones.filter((m) => m.status === 'done' || m.done_pct >= 100).length,
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
        {@const goalDone = rollupWord(project.goal_status)}
        <span class="flex items-center gap-2 truncate text-meta text-muted"
          >{project.goal.title}{#if goalDone}<Chip
              tone={goalDone.tone}
              icon={goalDone.icon}
              text={goalDone.text}
            />{/if}</span
        >
      {/if}
    </div>
    <div class="flex shrink-0 flex-col items-end" data-testid="progress-text">
      <span class="text-[1.75rem] leading-8 font-semibold tabular-nums">
        <span class="inline-block" use:changed={{ value: project.done_pct, cls: 'num' }}
          >{project.done_pct}%</span
        >
      </span>
      {#if project.claimed_pct > 0}
        <span class="text-meta text-muted tabular-nums">+{project.claimed_pct}% claimed</span>
      {/if}
    </div>
  </div>

  {#if project.forecast}
    <p class="-mt-3 self-end text-right text-meta" data-testid="forecast">
      {#if project.forecast.compact}
        <span title={project.forecast.help}>{project.forecast.text}</span>
        <span class="sr-only">{` (${project.forecast.help})`}</span>
      {:else}
        <span class="text-muted">{project.forecast.text}</span>
      {/if}
    </p>
  {/if}

  {#if project.run.finished}
    <p class="-mt-2 text-row font-medium text-ok" data-testid="run-finished">
      {project.run.finished.text}
    </p>
  {/if}

  <div class="flex flex-col gap-2">
    <p class="text-meta text-muted tabular-nums" data-testid="progress-caption">
      Done {project.done_pct}% · {complete} of {project.milestones.length}
      {project.milestones.length === 1 ? 'milestone' : 'milestones'} complete
    </p>
    <ProgressBar done={project.done_pct} claimed={project.claimed_pct} />
  </div>

  <MilestoneList {project} />

  {#if project.cost || project.leftovers.open > 0 || project.unestimated > 0}
    <div class="flex flex-wrap gap-x-4 border-t border-line pt-3 text-meta text-muted">
      {#if project.cost}<span class="tabular-nums" data-testid="cost">{project.cost.summary}</span
        >{/if}
      {#if project.leftovers.open > 0}
        <span
          >{project.leftovers.open}
          {project.leftovers.open === 1 ? 'leftover' : 'leftovers'}</span
        >
      {/if}
      {#if project.unestimated > 0}<span
          >{project.unestimated}
          {project.unestimated === 1 ? 'task' : 'tasks'} without an estimate</span
        >{/if}
    </div>
  {/if}
</li>
