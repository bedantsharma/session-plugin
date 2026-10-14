<script lang="ts">
  // done% in solid clay, claimed% as a clay hatch after it (≥ 3:1 stripes); the numbers
  // are always in text too.
  let {
    done,
    claimed,
    muted = false,
  }: { done: number; claimed: number; muted?: boolean } = $props()
</script>

<div
  class={['flex h-2 w-full overflow-hidden rounded-full border border-edge', muted && 'opacity-60']}
  role="img"
  aria-label={`${done}% done, ${claimed}% claimed`}
>
  <div class="h-full bg-clay" style:width={`${done}%`} data-testid="done-bar"></div>
  <div class="hatch h-full" style:width={`${claimed}%`} data-testid="claimed-bar"></div>
</div>
