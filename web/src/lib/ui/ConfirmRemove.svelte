<script lang="ts">
  // "Remove for good" (spec 2026-11-14 §4): the question in plain words from the server —
  // what goes, how much of it is done, who is working on it — and two buttons. The red
  // button stays off until the owner ticks that it can't be undone from the dashboard; the
  // server still refuses a stale click (the title changed, it is already gone).
  import type { ActResult } from '$lib/api'
  import * as Dialog from '$lib/components/ui/dialog'
  import { Button } from '$lib/components/ui/button'
  import type { OwnerAction, RemoveSpec } from '$lib/types'

  let {
    spec,
    onClose,
    act,
  }: {
    /** The thing to remove; the dialog is open while this is set. */
    spec: RemoveSpec | null
    onClose: () => void
    act: (a: OwnerAction) => Promise<ActResult>
  } = $props()

  let understood = $state(false)
  let pending = $state(false)
  let message = $state<string | null>(null)

  $effect(() => {
    if (spec) {
      understood = false
      pending = false
      message = null
    }
  })

  async function remove() {
    if (!spec || !understood || pending) return
    pending = true
    const res = await act({ ...spec.action, confirm: true })
    pending = false
    if (res.ok) onClose()
    else message = res.error
  }
</script>

<Dialog.Root open={spec !== null} onOpenChange={(o) => !o && onClose()}>
  <Dialog.Content>
    {#if spec}
      <Dialog.Header>
        <Dialog.Title>Remove for good?</Dialog.Title>
        <Dialog.Description data-testid="remove-impact">{spec.text}</Dialog.Description>
      </Dialog.Header>
      <label class="flex items-start gap-2 text-sm">
        <input type="checkbox" class="mt-0.5" bind:checked={understood} />
        <span>I understand this can't be undone from the dashboard.</span>
      </label>
      {#if message}
        <p class="text-meta text-warn" role="alert">{message}</p>
      {/if}
      <Dialog.Footer>
        <Button variant="outline" onclick={onClose}>Keep it</Button>
        <button
          type="button"
          class="inline-flex h-9 items-center justify-center rounded-lg border border-bad bg-bad px-4 text-sm font-medium text-on-accent transition-opacity disabled:opacity-40"
          disabled={!understood || pending}
          onclick={remove}>Remove for good</button
        >
      </Dialog.Footer>
    {/if}
  </Dialog.Content>
</Dialog.Root>
