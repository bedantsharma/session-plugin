<script lang="ts">
  // The owner's buttons for one thing (a task, a track): each ActionSpec as a button; a
  // spec with a required field (✗ Broken needs a note) opens a small dialog first. The
  // answer — or the server's refusal, in words — shows underneath.
  import type { ActResult } from '$lib/api'
  import ActionButton from '$lib/glance/ActionButton.svelte'
  import * as Dialog from '$lib/components/ui/dialog'
  import { Button } from '$lib/components/ui/button'
  import { Input } from '$lib/components/ui/input'
  import type { ActionSpec, OwnerAction } from '$lib/types'

  let {
    actions,
    act,
    compact = true,
  }: {
    actions: ActionSpec[]
    act: (a: OwnerAction) => Promise<ActResult>
    compact?: boolean
  } = $props()

  let pending = $state<string | null>(null)
  let message = $state<{ ok: boolean; text: string } | null>(null)
  let dialog = $state<{ spec: ActionSpec; values: Record<string, string> } | null>(null)

  async function run(spec: ActionSpec, extra: Record<string, string> = {}) {
    pending = spec.label
    message = null
    const res = await act({ ...spec.action, ...extra })
    pending = null
    message = res.ok ? { ok: true, text: res.reply } : { ok: false, text: res.error }
  }

  function click(spec: ActionSpec) {
    if (spec.inputs.some((i) => i.required)) {
      const values: Record<string, string> = {}
      for (const i of spec.inputs) values[i.name] = ''
      dialog = { spec, values }
      return
    }
    void run(spec)
  }

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    if (!dialog) return
    const { spec, values } = dialog
    const extra: Record<string, string> = {}
    for (const [k, v] of Object.entries(values)) if (v.trim()) extra[k] = v.trim()
    dialog = null
    await run(spec, extra)
  }
</script>

<span class="inline-flex flex-wrap items-center gap-2">
  {#each actions as spec (spec.label)}
    <ActionButton
      label={spec.label}
      tone={spec.tone}
      {compact}
      disabled={pending !== null}
      pending={pending === spec.label}
      onclick={() => click(spec)}
    />
  {/each}
  {#if message}
    <span class={['text-meta', message.ok ? 'text-muted' : 'text-warn']} role="status"
      >{message.text}</span
    >
  {/if}
</span>

<Dialog.Root open={dialog !== null} onOpenChange={(o) => !o && (dialog = null)}>
  <Dialog.Content>
    {#if dialog}
      <Dialog.Header>
        <Dialog.Title>{dialog.spec.label}</Dialog.Title>
      </Dialog.Header>
      <form class="flex flex-col gap-3" onsubmit={submit}>
        {#each dialog.spec.inputs as inp (inp.name)}
          <label class="flex flex-col gap-1 text-sm">
            {inp.label}
            <Input required={inp.required} bind:value={dialog.values[inp.name]} />
          </label>
        {/each}
        <Dialog.Footer>
          <Button type="button" variant="outline" onclick={() => (dialog = null)}>Cancel</Button>
          <Button type="submit">{dialog.spec.label}</Button>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>
