<script lang="ts">
  // A "⋯" menu: a button that opens a short list of actions. Escape and a click outside
  // close it; the list is a real menu for assistive tech.
  import type { Snippet } from 'svelte'

  let { label, children }: { label: string; children: Snippet<[() => void]> } = $props()

  let open = $state(false)
  let root: HTMLElement | undefined = $state()

  function close() {
    open = false
  }

  $effect(() => {
    if (!open) return
    const out = (e: MouseEvent) => {
      if (root && !root.contains(e.target as Node)) open = false
    }
    document.addEventListener('mousedown', out)
    return () => document.removeEventListener('mousedown', out)
  })
</script>

<div
  class="relative z-10 inline-block"
  bind:this={root}
  onkeydown={(e) => e.key === 'Escape' && (open = false)}
  role="presentation"
>
  <button
    type="button"
    class="grid size-8 place-items-center rounded-lg text-muted transition-colors duration-150 hover:bg-hover hover:text-fg"
    aria-haspopup="menu"
    aria-expanded={open}
    aria-label={label}
    onclick={() => (open = !open)}>⋯</button
  >
  {#if open}
    <div
      role="menu"
      class="absolute right-0 z-20 mt-1 flex min-w-44 flex-col rounded-lg border border-line bg-surface p-1 shadow-md"
    >
      {@render children(close)}
    </div>
  {/if}
</div>
