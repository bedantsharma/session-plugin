<script lang="ts">
  // A status chip (plan §2): text in the tone, a 1 px border in the tone, a 10% tint
  // fill, and always an icon or a word — never colour alone. With a tip, the chip takes
  // keyboard focus and its tooltip shows on focus as well as hover (aria-describedby).
  import type { ChipTone } from '$lib/words'

  let {
    tone,
    icon = '',
    text,
    tip,
    upper = false,
    live = false,
    class: extra = '',
  }: {
    tone: ChipTone
    icon?: string
    text: string
    tip?: string
    upper?: boolean
    /** A working session: a breathing dot instead of the icon. */
    live?: boolean
    class?: string
  } = $props()

  const tipId = $props.id()

  const look: Record<ChipTone, string> = {
    ok: 'border-ok text-ok bg-tint-ok',
    warn: 'border-warn text-warn bg-tint-warn',
    bad: 'border-bad text-bad bg-tint-bad',
    clay: 'border-clay text-clay',
    muted: 'border-edge text-muted',
  }
</script>

<!-- A chip with a tip is a tooltip trigger: it takes focus so keyboard users can read it. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<span
  class={[
    'chip relative inline-flex h-6 shrink-0 items-center gap-1 rounded-full border px-2.5 text-chip font-medium whitespace-nowrap',
    tip && 'cursor-help outline-none focus-visible:ring-3 focus-visible:ring-ring/50',
    upper && 'tracking-wide uppercase',
    look[tone],
    extra,
  ]}
  tabindex={tip ? 0 : undefined}
  aria-describedby={tip ? tipId : undefined}
  data-chip={tone}
>
  {#if live}<span class="working-dot mr-0.5" aria-hidden="true"></span>{:else if icon}<span
      aria-hidden="true">{icon}</span
    >{/if}{text}{#if tip}<span
      id={tipId}
      role="tooltip"
      class="chip-tip pointer-events-none absolute top-full left-0 z-20 mt-1 w-max max-w-[min(16rem,80vw)] rounded-md border border-edge bg-surface px-2 py-1 text-xs font-normal whitespace-normal text-fg normal-case shadow-sm"
      >{tip}</span
    >{/if}
</span>

<style>
  /* Out of layout at rest, so a tip near the edge never widens the page. */
  .chip-tip {
    display: none;
  }
  .chip:hover .chip-tip,
  .chip:focus-visible .chip-tip,
  .chip:focus .chip-tip {
    display: block;
  }
</style>
