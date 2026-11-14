<script lang="ts">
  // The "Hidden" drawer (spec 2026-11-14 §3): what the owner hid from the front screen,
  // grouped by kind, each with how long ago and a "Bring back" button. A thing also comes
  // back by itself when something new happens to it. It is not a screen: Escape closes it,
  // and it traps focus like a dialog while open.
  import type { ActResult } from '$lib/api'
  import * as Dialog from '$lib/components/ui/dialog'
  import { age } from '$lib/format'
  import type { HiddenItem, OwnerAction } from '$lib/types'

  let {
    items,
    open,
    onClose,
    act,
  }: {
    items: HiddenItem[]
    open: boolean
    onClose: () => void
    act: (a: OwnerAction) => Promise<ActResult>
  } = $props()

  const groups: { kind: HiddenItem['kind']; title: string }[] = [
    { kind: 'card', title: 'Needs you' },
    { kind: 'track', title: 'Tracks' },
    { kind: 'project', title: 'Projects' },
  ]
  let messages = $state<Record<string, string>>({})
  let pending = $state<string | null>(null)

  async function bringBack(item: HiddenItem) {
    pending = item.subject
    delete messages[item.subject]
    const res = await act(item.unhide)
    pending = null
    if (!res.ok) messages[item.subject] = res.error
  }
</script>

<Dialog.Root {open} onOpenChange={(o) => !o && onClose()}>
  <Dialog.Content
    class="fixed top-0 right-0 left-auto h-full max-h-none w-[min(26rem,100vw)] translate-x-0 translate-y-0 overflow-y-auto rounded-none rounded-l-xl"
    data-testid="hidden-drawer"
  >
    <Dialog.Header>
      <Dialog.Title>Hidden ({items.length})</Dialog.Title>
      <Dialog.Description
        >These stay out of the way until something new happens to them. Hiding never changes a
        number.</Dialog.Description
      >
    </Dialog.Header>
    {#if items.length === 0}
      <p class="text-row text-muted">Nothing is hidden.</p>
    {/if}
    {#each groups as g (g.kind)}
      {@const rows = items.filter((i) => i.kind === g.kind)}
      {#if rows.length}
        <section class="flex flex-col gap-2" aria-label={g.title}>
          <h3 class="text-meta font-medium text-muted uppercase">{g.title}</h3>
          <ul class="flex flex-col divide-y divide-line border-y border-line">
            {#each rows as item (item.subject)}
              <li class="flex flex-col gap-1 py-2" data-hidden={item.subject}>
                <div class="flex items-center gap-3">
                  <span class="min-w-0 flex-1 truncate text-row">{item.label}</span>
                  <button
                    type="button"
                    class="shrink-0 rounded-lg border border-edge px-3 py-1 text-sm transition-colors duration-150 hover:bg-hover disabled:opacity-50"
                    disabled={pending === item.subject}
                    onclick={() => void bringBack(item)}>Bring back</button
                  >
                </div>
                <span class="text-meta text-muted tabular-nums"
                  >{item.repo_name} · hidden {age(item.age_s)} ago</span
                >
                {#if messages[item.subject]}
                  <span class="text-meta text-warn" role="status">{messages[item.subject]}</span>
                {/if}
              </li>
            {/each}
          </ul>
        </section>
      {/if}
    {/each}
  </Dialog.Content>
</Dialog.Root>
