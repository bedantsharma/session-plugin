<script lang="ts">
  // The raw event stream with filters (spec §10.2), newest first, a page at a time,
  // grouped by day (plan §2).
  import { Input } from '$lib/components/ui/input'
  import { kindOptions, sentence, taskText } from './historyText'
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { HistoryPage } from '$lib/types'

  let { client, repo }: { client: Client; repo: string } = $props()

  let kind = $state('')
  let session = $state('')
  let task = $state('')
  let rows = $state<HistoryPage['events']>([])
  let kinds = $state<HistoryPage['kinds']>([])
  /** Rows whose raw details are open. */
  let open = $state<Record<number, boolean>>({})
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
      if (page.kinds) kinds = page.kinds
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
  <select
    class="h-9 w-56 rounded-lg border border-edge bg-surface px-3 text-sm"
    aria-label="kind of event"
    bind:value={kind}
  >
    <option value="">All kinds of event</option>
    {#each kindOptions(kinds) as k (k.kind)}
      <option value={k.kind}>{k.label}</option>
    {/each}
  </select>
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
          {@const task = taskText(e)}
          <li class="flex flex-col py-1.5 text-meta" data-event={e.pos}>
            <div class="flex min-h-7 items-center gap-3">
              <span class="w-14 shrink-0 text-muted tabular-nums">{hhmm(e.ts)}</span>
              <span class="min-w-0 flex-1 truncate text-row" title={sentence(e)}>{sentence(e)}</span
              >
              {#if task}<span class="hidden max-w-56 truncate text-muted md:inline" title={task}
                  >{task}</span
                >{/if}
              <button
                type="button"
                class="shrink-0 rounded-md border border-edge px-2 py-0.5 text-muted hover:bg-hover hover:text-fg"
                aria-expanded={!!open[e.pos]}
                aria-label={`${open[e.pos] ? 'Hide' : 'Show'} details of event ${e.pos}`}
                onclick={() => (open[e.pos] = !open[e.pos])}>details</button
              >
            </div>
            {#if open[e.pos]}
              <pre
                class="mt-1 overflow-x-auto rounded-md bg-surface p-2 font-mono text-muted"
                data-testid="raw">{JSON.stringify(
                  { kind: e.kind, session: e.session_id, task: e.task_id, data: e.data },
                  null,
                  2,
                )}</pre>
            {/if}
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
