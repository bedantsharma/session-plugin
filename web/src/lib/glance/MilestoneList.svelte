<script lang="ts">
  // A project's milestones (plan §2): one 36 px row each — id, title, mini bar, %, hill
  // and stuck chips — in shared columns; 6 shown, then "show all N". The bar grows from 7
  // to 12 rem and the title stops at 28 rem, so on wide cards the bar stays by its title;
  // below 640 px the bar hides and the chips drop under the title. A milestone a working
  // session is on gets a slow sheen on its bar.
  import ProgressBar from './ProgressBar.svelte'
  import Chip from '$lib/ui/Chip.svelte'
  import { changed } from '$lib/changed'
  import { milestoneHill, rollupWord } from '$lib/words'
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
  <ul
    class="-mt-2 grid grid-cols-1 gap-x-3 sm:grid-cols-[2.5rem_minmax(0,28rem)_minmax(7rem,12rem)_3rem_auto]"
  >
    {#each milestones as m (m.id)}
      {@const hill = milestoneHill(m)}
      {@const roll = rollupWord(m.status)}
      <li
        class="-mx-2 grid grid-cols-[2.5rem_minmax(0,1fr)_3rem] items-center gap-x-3 rounded-lg px-2 py-1.5 sm:col-span-5 sm:h-9 sm:grid-cols-subgrid sm:py-0"
        data-milestone={m.id}
        use:changed={{ value: [m.done_pct, m.claimed_pct, m.hill, m.stuck] }}
      >
        <span
          class="font-mono text-meta text-muted"
          title={m.shown !== m.id ? `id ${m.id} for agents and the command line` : undefined}
          >{m.shown}</span
        >
        <span class="truncate text-row">{m.title}</span>
        <div class="hidden sm:block">
          <ProgressBar
            done={m.done_pct}
            claimed={m.claimed_pct}
            size="mini"
            labelled={false}
            active={active.has(m.id)}
          />
        </div>
        <span
          class={['text-right text-meta tabular-nums', m.hill === 'uphill' && 'text-muted']}
          data-testid={m.hill === 'uphill' ? 'exploring-pct' : undefined}>{m.done_pct}%</span
        >
        <span
          class="col-span-2 col-start-2 flex flex-wrap items-center gap-1.5 sm:col-span-1 sm:col-start-auto"
        >
          {#if roll}<Chip tone={roll.tone} icon={roll.icon} text={roll.text} />{/if}
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
