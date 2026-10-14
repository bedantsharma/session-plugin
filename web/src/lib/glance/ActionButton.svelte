<script lang="ts">
  // One needs-you button, styled by its tone. Real <button>s with their label as text.
  import { Button } from '$lib/components/ui/button'
  import type { Tone } from '$lib/types'

  let {
    label,
    tone,
    disabled = false,
    pending = false,
    onclick,
  }: {
    label: string
    tone: Tone
    disabled?: boolean
    pending?: boolean
    onclick: () => void
  } = $props()

  const look: Record<Tone, string> = {
    primary: 'border-fg',
    neutral: 'border-edge bg-transparent text-fg hover:bg-line',
    danger: 'border-bad bg-transparent text-bad hover:bg-line',
  }
</script>

<Button
  size="sm"
  variant={tone === 'primary' ? 'default' : 'outline'}
  class={look[tone]}
  {disabled}
  aria-busy={pending}
  {onclick}
>
  {pending ? `${label}…` : label}
</Button>
