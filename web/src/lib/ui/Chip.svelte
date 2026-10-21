<script lang="ts">
  // A status chip (plan §2): text in the tone, a 1 px border in the tone, a 10% tint
  // fill, and always an icon or a word — never colour alone.
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

  const look: Record<ChipTone, string> = {
    ok: 'border-ok text-ok bg-tint-ok',
    warn: 'border-warn text-warn bg-tint-warn',
    bad: 'border-bad text-bad bg-tint-bad',
    clay: 'border-clay text-clay',
    muted: 'border-edge text-muted',
  }
</script>

<span
  class={[
    'inline-flex h-6 shrink-0 items-center gap-1 rounded-full border px-2.5 text-chip font-medium whitespace-nowrap',
    upper && 'tracking-wide uppercase',
    look[tone],
    extra,
  ]}
  title={tip}
  data-chip={tone}
>
  {#if live}<span class="working-dot mr-0.5" aria-hidden="true"></span>{:else if icon}<span
      aria-hidden="true">{icon}</span
    >{/if}{text}{#if tip}<span class="sr-only">: {tip}</span>{/if}
</span>
