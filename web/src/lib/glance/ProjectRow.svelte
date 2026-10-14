<script lang="ts">
  // One project (spec §10.1, §5.6): done% bar with claimed% apart, uphill milestones as a
  // text pill with their % muted, stuck badges, leftovers.
  import ProgressBar from './ProgressBar.svelte'
  import type { ProjectRow } from '$lib/types'

  let { project, onOpen }: { project: ProjectRow; onOpen?: (repo: string) => void } = $props()
</script>

<li class="flex flex-col gap-1 rounded-md px-2 py-1.5" data-project={project.repo_id}>
  <div class="flex items-baseline gap-2 text-sm">
    <button
      type="button"
      class="font-medium hover:underline"
      onclick={() => onOpen?.(project.repo_id)}>{project.name}</button
    >
    {#if project.goal}<span class="truncate text-muted">{project.goal.title}</span>{/if}
    <span class="ml-auto shrink-0 text-xs" data-testid="progress-text">
      {project.done_pct}% done{#if project.claimed_pct > 0}&nbsp;· +{project.claimed_pct}% claimed{/if}
    </span>
  </div>
  <ProgressBar done={project.done_pct} claimed={project.claimed_pct} />
  <div class="flex flex-wrap gap-x-3 gap-y-0.5 text-xs">
    {#each project.milestones as m (m.id)}
      <span class="flex items-center gap-1" data-milestone={m.id}>
        <span class="text-muted">{m.id}</span>
        {m.title}
        {#if m.hill === 'uphill'}
          <span class="rounded-full border border-warn px-1.5 text-warn">▲ uphill</span>
          <span class="text-muted" data-testid="uphill-pct">{m.done_pct}%</span>
        {:else}
          {#if m.hill === 'downhill'}<span class="rounded-full border border-ok px-1.5 text-ok"
              >▼ downhill</span
            >{/if}
          <span>{m.done_pct}%</span>
        {/if}
        {#if m.stuck}
          <span class="rounded-full border border-warn px-1.5 text-warn"
            >stuck · {m.stuck_sessions} sessions</span
          >
        {/if}
      </span>
    {/each}
    {#if project.leftovers.open > 0}
      <span class="text-muted">leftovers: {project.leftovers.open}</span>
    {/if}
    {#if project.unestimated > 0}
      <span class="text-muted">{project.unestimated} unestimated</span>
    {/if}
  </div>
</li>
