<script lang="ts">
  // The raw event stream with filters (spec §10.2), newest first, a page at a time,
  // grouped by day (plan §2).
  import { Input } from '$lib/components/ui/input'
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { HistoryPage } from '$lib/types'

  let { client, repo }: { client: Client; repo: string } = $props()

  let kind = $state('')
  let session = $state('')
  let task = $state('')
  let rows = $state<HistoryPage['events']>([])
  let next = $state<number | null>(null)
  let error = $state<string | null>(null)

  async function load(before: number | null) {
    const params: Record<string, string> = { limit: '100' }
    if (kind) params.kind = kind
    if (session) params.session = session
    if (task) params.task = task
    if (before !== null) params.before = String(before)
    const q = Object.entries(params)
      .map(([k, v]) => `${k}=${encodeURIComponent(v)}`)
      .join('&')
    try {
      const page = await client.get<HistoryPage>(`/api/history/${encodeURIComponent(repo)}?${q}`)
      rows = before === null ? page.events : [...rows, ...page.events]
      next = page.next_before
      error = null
    } catch (e) {
      error = (e as Error).message
    }
  }

  // Grouped by local day, newest first.
  const days = $derived.by(() => {
    const out: [string, HistoryPage['events']][] = []
    for (const e of rows) {
      const day = new Date(e.ts).toLocaleDateString(undefined, {
        weekday: 'long',
        day: 'numeric',
        month: 'long',
      })
      const last = out.at(-1)
      if (last && last[0] === day) last[1].push(e)
      else out.push([day, [e]])
    }
    return out
  })

  $effect(() => {
    void [kind, session, task]
    void load(null)
  })
</script>

<h2 class="text-xl font-semibold">History</h2>
<div class="flex flex-wrap gap-2">
  <Input class="h-9 w-48 text-sm" placeholder="kind" aria-label="kind" bind:value={kind} />
  <Input class="h-9 w-48 text-sm" placeholder="session" aria-label="session" bind:value={session} />
  <Input class="h-9 w-48 text-sm" placeholder="task" aria-label="task" bind:value={task} />
</div>
{#if error}<p class="text-row text-warn">{error}</p>{/if}
<div class="flex flex-col">
  {#each days as [day, events] (day)}
    <section class="flex flex-col gap-3 pt-6 first:pt-0">
      <h3 class="text-base font-semibold">{day}</h3>
      <ol class="flex flex-col divide-y divide-line border-y border-line">
        {#each events as e (e.pos)}
          <li
            class="grid h-9 grid-cols-[3.5rem_12rem_6rem_minmax(0,1fr)] items-center gap-3 text-meta"
          >
            <span class="text-muted tabular-nums">{hhmm(e.ts)}</span>
            <span class="truncate font-mono">{e.kind}</span>
            <span class="truncate font-mono text-muted"
              >{e.session_id.slice(0, 6)} {e.task_id ?? ''}</span
            >
            <span class="truncate font-mono text-muted" title={JSON.stringify(e.data)}
              >{JSON.stringify(e.data)}</span
            >
          </li>
        {/each}
      </ol>
    </section>
  {:else}
    {#if !error}<p class="text-row text-muted">No events match.</p>{/if}
  {/each}
</div>
{#if next !== null}
  <button
    type="button"
    class="inline-flex h-9 items-center self-start rounded-lg border border-edge px-4 text-sm font-medium hover:bg-hover"
    onclick={() => load(next)}>Older</button
  >
{/if}
