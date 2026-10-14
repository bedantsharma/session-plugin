<script lang="ts">
  // Project drill-down (spec §10.2): plan tree with intents collapsed, levels and stale
  // badges, estimate vs actual, dependencies, the burn-up of the scope line (added scope
  // shows as steps up the total line, listed with their times) and re-plan history.
  import { LineChart } from 'layerchart'
  import * as Collapsible from '$lib/components/ui/collapsible'
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { ProjectDrill } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let {
    client,
    repo,
    version,
    onOpenTask,
    onOpenHistory,
    onOpenTuning,
  }: {
    client: Client
    repo: string
    version: number
    onOpenTask: (repo: string, id: string) => void
    onOpenHistory: (repo: string) => void
    onOpenTuning: (repo: string) => void
  } = $props()

  const m = useModel<ProjectDrill>(
    () => client,
    () => `/api/project/${encodeURIComponent(repo)}`,
    () => version,
  )

  // Steps: hold each value until the next change.
  const points = $derived.by(() => {
    const out: { date: Date; total: number; done: number }[] = []
    for (const p of m.data?.scope ?? []) {
      const date = new Date(p.ts)
      const last = out.at(-1)
      if (last) out.push({ date, total: last.total, done: last.done })
      out.push({ date, total: p.total_min, done: p.done_min })
    }
    return out
  })
  const added = $derived(
    (m.data?.scope ?? []).flatMap((p, i, all) =>
      i > 0 && p.total_min > all[i - 1].total_min
        ? [{ ts: p.ts, by: p.total_min - all[i - 1].total_min }]
        : [],
    ),
  )
</script>

{#if m.error}
  <p class="text-warn">{m.error}</p>
{:else if m.data}
  <div class="flex items-baseline gap-3">
    <h2 class="text-lg font-medium">{m.data.name}</h2>
    <button
      type="button"
      class="text-xs text-muted hover:underline"
      onclick={() => onOpenHistory(repo)}>history</button
    >
    <button
      type="button"
      class="text-xs text-muted hover:underline"
      onclick={() => onOpenTuning(repo)}>drift tuning</button
    >
  </div>
  {#if points.length > 1}
    <div class="mt-3 h-48" aria-label="Scope line: total live estimate and done, over time">
      <LineChart
        data={points}
        x="date"
        series={[
          { key: 'total', label: 'scope (min)', color: 'var(--muted)' },
          { key: 'done', label: 'done (min)', color: 'var(--clay)' },
        ]}
        legend
      />
    </div>
    {#if added.length}
      <p class="mt-1 text-xs text-muted">
        scope added: {added.map((a) => `+${a.by}m at ${hhmm(a.ts)}`).join(' · ')}
      </p>
    {/if}
  {/if}
  <ul class="mt-4 flex flex-col">
    {#each m.data.tree as n (n.id)}
      <li style:padding-left={`${n.depth * 1.25}rem`} class="py-0.5 text-sm">
        <Collapsible.Root>
          <div class="flex items-baseline gap-2">
            <span class="w-10 shrink-0 font-mono text-xs text-muted">{n.id}</span>
            {#if n.kind === 'task' || n.kind === 'subtask'}
              <button type="button" class="hover:underline" onclick={() => onOpenTask(repo, n.id)}
                >{n.title}</button
              >
            {:else}
              <span class="font-medium">{n.title}</span>
            {/if}
            <span class="rounded border border-edge px-1 text-xs">{n.status}</span>
            {#if n.stale}<span class="rounded border border-warn px-1 text-xs text-warn">stale</span
              >{/if}
            {#if n.owner_checked}<span class="text-xs text-clay">✓ owner</span>{/if}
            {#if n.hill}<span class="text-xs text-warn">{n.hill}</span>{/if}
            {#if n.estimate_min !== null || n.actual_min > 0}
              <span class="text-xs text-muted">{n.actual_min}m of {n.estimate_min ?? '?'}m</span>
            {/if}
            {#if n.depends_on.length}<span class="text-xs text-muted"
                >after {n.depends_on.join(', ')}</span
              >{/if}
            {#if n.decisions}<span class="text-xs text-muted">{n.decisions} decisions</span>{/if}
            {#if n.intent}
              <Collapsible.Trigger class="text-xs text-muted hover:underline"
                >intent</Collapsible.Trigger
              >
            {/if}
          </div>
          {#if n.intent}
            <Collapsible.Content class="ml-12 text-xs text-muted">{n.intent}</Collapsible.Content>
          {/if}
        </Collapsible.Root>
      </li>
    {/each}
  </ul>
  {#if m.data.replans.length}
    <h3 class="mt-4 text-sm font-medium">Re-plans</h3>
    <ul class="text-xs text-muted">
      {#each m.data.replans as r (r.ts)}
        <li>{hhmm(r.ts)} · {r.op_count} ops · {r.reason ?? ''}</li>
      {/each}
    </ul>
  {/if}
{:else}
  <p class="text-muted">Loading…</p>
{/if}
