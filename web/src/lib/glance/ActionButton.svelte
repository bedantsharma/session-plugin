<script lang="ts">
  // One needs-you button, styled by its tone. Real <button>s with their label as text;
  // while the server answers, a small spinner and "…" (the spinner stops under reduced
  // motion and the "…" stays).
  import type { Tone } from '$lib/types'

  let {
    label,
    tone,
    disabled = false,
    pending = false,
    compact = false,
    onclick,
  }: {
    label: string
    tone: Tone
    disabled?: boolean
    pending?: boolean
    compact?: boolean
    onclick: () => void
  } = $props()

  const look: Record<Tone, string> = {
    primary: 'border-fg bg-fg text-bg hover:opacity-90',
    neutral: 'border-edge bg-transparent text-fg hover:bg-hover',
    danger: 'border-bad bg-transparent text-bad hover:bg-tint-bad',
  }
</script>

<button
  type="button"
  class={[
    'inline-flex shrink-0 items-center justify-center gap-2 rounded-lg border font-medium whitespace-nowrap transition-colors duration-150 outline-none focus-visible:ring-3 focus-visible:ring-ring/50 disabled:pointer-events-none disabled:opacity-50',
    compact ? 'h-8 px-3 text-sm' : 'h-9 px-4 text-sm',
    look[tone],
  ]}
  {disabled}
  aria-busy={pending}
  {onclick}
>
  {#if pending}<span class="spinner" aria-hidden="true"></span>{/if}
  {pending ? `${label}…` : label}
</button>
