<script lang="ts">
  // Branch groups (spec §10.1): the same branch with live sessions in two or more repos,
  // one line each — the mono branch name, the combined bar and "2 repos", with each
  // repo's % in the tooltip. Nothing renders without groups.
  import ProgressBar from './ProgressBar.svelte'
  import { groupPct, reposWord } from '$lib/words'
  import type { BranchGroup } from '$lib/types'

  let { groups }: { groups: BranchGroup[] } = $props()

  const tip = (g: BranchGroup) =>
    g.repos.map((r) => `${r.name} ${groupPct(r.done_pct)}`).join(' · ')
</script>

{#if groups.length}
  <section aria-label="Branch groups" class="flex flex-col gap-2" data-testid="branch-groups">
    <h2 class="text-base font-semibold">Same branch, several repos</h2>
    <ul class="flex flex-col gap-2">
      {#each groups as g (g.branch)}
        <li
          class="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1 text-row"
          title={tip(g)}
          data-branch={g.branch}
        >
          <span class="max-w-40 min-w-0 truncate font-mono text-meta">{g.branch}</span>
          <div class="min-w-24 flex-1">
            <ProgressBar
              done={g.done_pct ?? 0}
              claimed={g.claimed_pct ?? 0}
              muted={g.done_pct === null}
            />
          </div>
          <span class="min-w-0 text-meta text-muted tabular-nums"
            >{groupPct(g.done_pct)} · {reposWord(g.repos.length)}</span
          >
          <span class="sr-only">{tip(g)}</span>
        </li>
      {/each}
    </ul>
  </section>
{/if}
