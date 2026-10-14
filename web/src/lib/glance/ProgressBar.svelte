<script lang="ts">
  // done% in solid clay, claimed% as a clay hatch after it (the stripes keep clay's 3:1
  // against the track); the numbers are always in text too. Mini bars (milestone rows)
  // are hidden from assistive tech because their % is the next cell.
  let {
    done,
    claimed = 0,
    muted = false,
    size = 'md',
    active = false,
    labelled = true,
  }: {
    done: number
    claimed?: number
    muted?: boolean
    size?: 'md' | 'card' | 'mini'
    /** A working session is on it: a slow sheen sweeps the fill. */
    active?: boolean
    labelled?: boolean
  } = $props()

  const height = { md: 'h-2.5', card: 'h-2', mini: 'h-1.5' }
</script>

<div
  class={[
    'relative flex w-full overflow-hidden rounded-full bg-line',
    height[size],
    muted && 'opacity-60',
  ]}
  role={labelled ? 'img' : undefined}
  aria-label={labelled ? `${done}% done, ${claimed}% claimed` : undefined}
  aria-hidden={labelled ? undefined : 'true'}
>
  <div
    class={['bar-fill relative h-full overflow-hidden bg-clay', active && 'sheen']}
    style:width={`${done}%`}
    data-testid={labelled ? 'done-bar' : undefined}
  ></div>
  {#if claimed > 0 || labelled}
    <div
      class="bar-fill hatch h-full"
      style:width={`${claimed}%`}
      data-testid={labelled ? 'claimed-bar' : undefined}
    ></div>
  {/if}
</div>
