<script lang="ts">
  // A project's milestones (plan §2): one 36 px row each — id, title, mini bar, %, hill
  // and stuck chips — in shared columns; 6 shown, then "show all N". A milestone a working
  // session is on gets a slow sheen on its bar.
  import ProgressBar from './ProgressBar.svelte'
  import Chip from '$lib/ui/Chip.svelte'
  import { hillWord } from '$lib/words'
  import type { ProjectRow } from '$lib/types'

  let { project, limit = 6 }: { project: ProjectRow; limit?: number } = $props()

  let all = $state(false)
  const milestones = $derived(all ? project.milestones : project.milestones.slice(0, limit))
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
          {#if hill}<Chip tone={hill.tone} icon={hill.icon} text={hill.text} tip={hill.tip} />{/if}
          {#if m.stuck}
            <Chip tone="warn" icon="!" text={`stuck · ${m.stuck_sessions} sessions`} />
          {/if}
        </span>
      </li>
    {/each}
  </ul>
  {#if project.milestones.length > limit}
    <button
      type="button"
      class="relative z-10 -mt-3 self-start rounded-lg px-2 py-1 text-sm font-medium text-muted hover:bg-hover hover:text-fg"
      onclick={() => (all = !all)}
      >{all ? `show the first ${limit}` : `show all ${project.milestones.length}`}</button
    >
  {/if}
{/if}
