<script lang="ts">
  // One open track under a project (spec 2026-11-14 §2): its name and goal, its own big
  // done %, its forecast at its own pace, a captioned bar (with how many tasks count as
  // done without tests), its milestones, and a "⋯" menu to hide the bar, finish the track
  // or remove it for good. Hiding never changes a number.
  import type { ActResult } from '$lib/api'
  import { changed } from '$lib/changed'
  import ProgressBar from './ProgressBar.svelte'
  import MilestoneList from './MilestoneList.svelte'
  import ConfirmRemove from '$lib/ui/ConfirmRemove.svelte'
  import RowMenu from '$lib/ui/RowMenu.svelte'
  import Chip from '$lib/ui/Chip.svelte'
  import { rollupWord } from '$lib/words'
  import type { OwnerAction, RemoveSpec, SessionRow, TrackRow } from '$lib/types'

  let {
    track,
    repo,
    sessions = [],
    act,
  }: {
    track: TrackRow
    repo: string
    sessions?: SessionRow[]
    act?: (a: OwnerAction) => Promise<ActResult>
  } = $props()

  let message = $state<{ ok: boolean; text: string } | null>(null)
  let removing = $state<RemoveSpec | null>(null)

  const complete = $derived(
    track.milestones.filter((m) => m.status === 'done' || m.done_pct >= 100).length,
  )
  const goalDone = $derived(rollupWord(track.goal_status))

  async function send(action: OwnerAction) {
    if (!act) return
    message = null
    const res = await act(action)
    message = res.ok ? { ok: true, text: res.reply } : { ok: false, text: res.error }
  }

  const hide = () =>
    send({
      kind: 'hide',
      repo,
      subject: track.subject,
      label: track.goal.title,
      stamp: track.stamp,
    })
</script>

<li class="flex flex-col gap-3 rounded-lg border border-line p-4" data-track={track.track}>
  <div class="flex items-start justify-between gap-3">
    <div class="flex min-w-0 flex-col">
      <span class="text-meta text-muted">Track {track.track}</span>
      <span class="flex items-center gap-2 truncate text-row font-medium"
        >{track.goal.title}{#if goalDone}<Chip
            tone={goalDone.tone}
            icon={goalDone.icon}
            text={goalDone.text}
          />{/if}</span
      >
    </div>
    <div class="flex shrink-0 items-start gap-2">
      <div class="flex flex-col items-end" data-testid="track-progress">
        <span class="text-[1.5rem] leading-7 font-semibold tabular-nums">
          <span class="inline-block" use:changed={{ value: track.done_pct, cls: 'num' }}
            >{track.done_pct}%</span
          >
        </span>
        {#if track.claimed_pct > 0}
          <span class="text-meta text-muted tabular-nums">+{track.claimed_pct}% claimed</span>
        {/if}
      </div>
      {#if act}
        <RowMenu label={`Menu for track ${track.track}`}>
          {#snippet children(close)}
            <button
              type="button"
              role="menuitem"
              class="rounded-md px-3 py-1.5 text-left text-sm hover:bg-hover"
              onclick={() => {
                close()
                void hide()
              }}>Hide this track</button
            >
            <button
              type="button"
              role="menuitem"
              class="rounded-md px-3 py-1.5 text-left text-sm hover:bg-hover"
              onclick={() => {
                close()
                void send(track.finish.action)
              }}>Finish track</button
            >
            {#if track.remove}
              <button
                type="button"
                role="menuitem"
                class="rounded-md px-3 py-1.5 text-left text-sm text-bad hover:bg-tint-bad"
                onclick={() => {
                  close()
                  removing = track.remove
                }}>Remove for good</button
              >
            {/if}
          {/snippet}
        </RowMenu>
      {/if}
    </div>
  </div>

  {#if track.forecast}
    <p class="text-meta" data-testid="track-forecast">
      {#if track.forecast.compact}
        <span title={track.forecast.help}>{track.forecast.text}</span>
      {:else}
        <span class="text-muted">{track.forecast.text}</span>
      {/if}
    </p>
  {/if}

  {#if track.run.finished}
    <p class="text-row font-medium text-ok">{track.run.finished.text}</p>
  {/if}

  <div class="flex flex-col gap-2">
    <p class="text-meta text-muted tabular-nums" data-testid="track-caption">
      Done {track.done_pct}% · {complete} of {track.milestones.length}
      {track.milestones.length === 1 ? 'milestone' : 'milestones'} complete{#if track.reviewed_tasks > 0}
        · includes {track.reviewed_tasks}
        {track.reviewed_tasks === 1 ? 'task' : 'tasks'} done without tests{/if}
    </p>
    <ProgressBar done={track.done_pct} claimed={track.claimed_pct} />
  </div>

  <MilestoneList project={{ milestones: track.milestones, sessions }} />

  {#if message}
    <p class={['text-meta', message.ok ? 'text-muted' : 'text-warn']} role="status">
      {message.text}
    </p>
  {/if}
</li>

{#if act}
  <ConfirmRemove spec={removing} onClose={() => (removing = null)} {act} />
{/if}
