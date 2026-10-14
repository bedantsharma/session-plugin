<script lang="ts">
  // The needs-you queue (spec §10.1): only items with an owner response, most urgent
  // first, buttons straight from the server's `actions`. Dialogs only where input is
  // needed; Continue takes an optional inline reason; Undo has no confirm (it stashes, and
  // the reply says how to get the files back). After a click the button waits for the
  // server: no optimistic removal. While the pointer or focus is inside, updates don't
  // move rows; they apply when it leaves.
  import ActionButton from './ActionButton.svelte'
  import * as Dialog from '$lib/components/ui/dialog'
  import { Input } from '$lib/components/ui/input'
  import { Button } from '$lib/components/ui/button'
  import type { ActResult } from '$lib/api'
  import { age } from '$lib/format'
  import type { ActionSpec, NeedsYou, OwnerAction } from '$lib/types'

  let {
    items,
    act,
    disabled = false,
    showAll = $bindable(false),
    onOpenSession,
  }: {
    items: NeedsYou[]
    act: (a: OwnerAction) => Promise<ActResult>
    disabled?: boolean
    showAll?: boolean
    onOpenSession?: (id: string) => void
  } = $props()

  /** Shown without "show all". */
  const URGENT = 4

  let pointerInside = $state(false)
  let focusInside = $state(false)
  let shown = $state<NeedsYou[]>([])
  $effect(() => {
    const next = items
    if (!pointerInside && !focusInside) shown = next
  })

  let pending = $state<Record<string, string | null>>({})
  let messages = $state<Record<string, { ok: boolean; text: string }>>({})
  let reasons = $state<Record<string, string>>({})
  let dialog: { item: NeedsYou; spec: ActionSpec; values: Record<string, string> } | null =
    $state(null)

  const kindWord: Record<string, string> = {
    drift_red: 'drift',
    question: 'question',
    permission_wait: 'waiting',
    verify_overdue: 'verify',
    leftover: 'leftovers',
  }

  async function run(item: NeedsYou, spec: ActionSpec, extra: Record<string, unknown> = {}) {
    pending[item.id] = spec.label
    delete messages[item.id]
    const res = await act({ ...spec.action, ...extra })
    pending[item.id] = null
    messages[item.id] = res.ok ? { ok: true, text: res.reply } : { ok: false, text: res.error }
  }

  function click(item: NeedsYou, spec: ActionSpec) {
    if (spec.inputs.some((i) => i.required)) {
      const values: Record<string, string> = {}
      for (const i of spec.inputs) {
        const v = spec.action[i.name]
        values[i.name] = typeof v === 'string' ? v : ''
      }
      dialog = { item, spec, values }
      return
    }
    const optional = spec.inputs.filter((i) => !i.required)
    const extra: Record<string, string> = {}
    for (const i of optional) {
      const v = (reasons[`${item.id}:${i.name}`] ?? '').trim()
      if (v) extra[i.name] = v
    }
    void run(item, spec, extra)
  }

  async function submitDialog(e: SubmitEvent) {
    e.preventDefault()
    if (!dialog) return
    const { item, spec, values } = dialog
    const extra: Record<string, string> = {}
    for (const [k, v] of Object.entries(values)) if (v.trim()) extra[k] = v.trim()
    dialog = null
    await run(item, spec, extra)
  }

  function copy(text: string) {
    void navigator.clipboard?.writeText(text)
  }

  const visible = $derived(showAll ? shown : shown.slice(0, URGENT))
</script>

{#if shown.length > 0}
  <section
    aria-label="Needs you"
    class="rounded-lg border-2 border-bad bg-surface p-2"
    onpointerenter={() => (pointerInside = true)}
    onpointerleave={() => (pointerInside = false)}
    onfocusin={() => (focusInside = true)}
    onfocusout={(e) => {
      const to = e.relatedTarget as Node | null
      if (!to || !(e.currentTarget as HTMLElement).contains(to)) focusInside = false
    }}
  >
    <ul class="flex flex-col gap-1">
      {#each visible as item (item.id)}
        <li
          class="flex flex-wrap items-center gap-x-3 gap-y-1 rounded-md px-2 py-1.5"
          data-item={item.id}
        >
          <span class="w-20 shrink-0 text-xs font-medium tracking-wide text-bad uppercase">
            ■ {kindWord[item.kind] ?? item.kind}
          </span>
          <div class="min-w-0 flex-1">
            <div class="truncate text-sm">
              {#if item.session_id && onOpenSession}
                <button
                  type="button"
                  class="hover:underline"
                  onclick={() => onOpenSession?.(item.session_id as string)}>{item.title}</button
                >
              {:else}
                {item.title}
              {/if}
              <span class="text-xs text-muted">· {item.repo_name} · {age(item.age_s)}</span>
            </div>
            <div class="truncate text-xs text-muted">{item.detail}</div>
            {#if item.files?.length}
              <div class="truncate font-mono text-xs text-muted">{item.files.join(' ')}</div>
            {/if}
            {#if messages[item.id]}
              <div
                class={['text-xs', messages[item.id].ok ? 'text-ok' : 'text-warn']}
                role="status"
              >
                {messages[item.id].text}
              </div>
            {/if}
          </div>
          {#if item.actions.length}
            <div class="flex items-center gap-1.5">
              {#each item.actions as spec (spec.label)}
                {#each spec.inputs.filter((i) => !i.required) as inp (inp.name)}
                  <Input
                    class="h-8 w-40 text-xs"
                    placeholder={inp.label}
                    aria-label={`${spec.label}: ${inp.label}`}
                    disabled={disabled || !!pending[item.id]}
                    bind:value={reasons[`${item.id}:${inp.name}`]}
                  />
                {/each}
                <ActionButton
                  label={spec.label}
                  tone={spec.tone}
                  disabled={disabled || !!pending[item.id]}
                  pending={pending[item.id] === spec.label}
                  onclick={() => click(item, spec)}
                />
              {/each}
            </div>
          {:else if item.respond_in}
            <div class="flex items-center gap-1.5 text-xs text-muted">
              answer in <span class="font-mono">S {item.respond_in.session_short}</span>
              {#if item.respond_in.worktree}
                · <span class="font-mono">{item.respond_in.worktree}</span>
                <Button
                  size="xs"
                  variant="outline"
                  class="border-edge"
                  aria-label="Copy the worktree path"
                  onclick={() => copy(item.respond_in?.worktree ?? '')}>copy</Button
                >
              {/if}
            </div>
          {/if}
        </li>
      {/each}
    </ul>
    {#if shown.length > URGENT}
      <button
        type="button"
        class="mt-1 px-2 text-xs text-muted hover:underline"
        onclick={() => (showAll = !showAll)}
      >
        {showAll ? 'show the 4 most urgent' : `show all ${shown.length}`}
      </button>
    {/if}
  </section>
{/if}

<Dialog.Root open={dialog !== null} onOpenChange={(o) => !o && (dialog = null)}>
  <Dialog.Content>
    {#if dialog}
      <Dialog.Header>
        <Dialog.Title>{dialog.spec.label}</Dialog.Title>
        <Dialog.Description>{dialog.item.title}</Dialog.Description>
      </Dialog.Header>
      <form class="flex flex-col gap-3" onsubmit={submitDialog}>
        {#each dialog.spec.inputs as inp (inp.name)}
          <label class="flex flex-col gap-1 text-sm">
            {inp.label}
            <Input required={inp.required} bind:value={dialog.values[inp.name]} />
          </label>
        {/each}
        <Dialog.Footer>
          <Button type="submit">{dialog.spec.label}</Button>
        </Dialog.Footer>
      </form>
    {/if}
  </Dialog.Content>
</Dialog.Root>
