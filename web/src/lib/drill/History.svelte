<script lang="ts">
  // The raw event stream with filters (spec §10.2), newest first, a page at a time.
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

  $effect(() => {
    void [kind, session, task]
    void load(null)
  })
</script>

<h2 class="text-lg font-medium">History</h2>
<div class="mt-2 flex gap-2">
  <Input class="h-8 w-40" placeholder="kind" aria-label="kind" bind:value={kind} />
  <Input class="h-8 w-40" placeholder="session" aria-label="session" bind:value={session} />
  <Input class="h-8 w-40" placeholder="task" aria-label="task" bind:value={task} />
</div>
{#if error}<p class="text-warn">{error}</p>{/if}
<ol class="mt-2 font-mono text-meta">
  {#each rows as e (e.pos)}
    <li class="truncate">
      <span class="text-muted">{hhmm(e.ts)}</span>
      {e.kind}
      <span class="text-muted">{e.session_id.slice(0, 6)} {e.task_id ?? ''}</span>
      {JSON.stringify(e.data)}
    </li>
  {/each}
</ol>
{#if next !== null}
  <button type="button" class="mt-2 text-meta text-muted hover:underline" onclick={() => load(next)}
    >older</button
  >
{/if}
