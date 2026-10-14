<script lang="ts">
  // Task drill-down (spec §10.2): decisions with superseded-by chains, verification
  // evidence, actors and retries.
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { TaskDrill } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let { client, repo, id, version }: { client: Client; repo: string; id: string; version: number } =
    $props()

  const m = useModel<TaskDrill>(
    () => client,
    () => `/api/task/${encodeURIComponent(repo)}/${encodeURIComponent(id)}`,
    () => version,
  )
</script>

{#if m.error}
  <p class="text-warn">{m.error}</p>
{:else if m.data}
  {@const t = m.data}
  <h2 class="text-lg font-medium">{t.id} {t.title}</h2>
  <p class="text-xs text-muted">
    {t.status ?? ''} · {t.actual_min}m of {t.estimate_min ?? '?'}m · {t.retries} retries
    {#if t.actors.length}· {t.actors
        .map((a) => [a.agent_type, a.model].filter(Boolean).join(' '))
        .join(', ')}{/if}
  </p>
  <h3 class="mt-3 text-sm font-medium">Decisions</h3>
  {#each t.decisions as chain (chain[0].decision_id)}
    <ol class="mb-2 text-sm">
      {#each chain as d (d.decision_id)}
        <li class={d.superseded_by ? 'text-muted line-through' : ''}>
          <span class="font-mono text-xs">{d.decision_id}</span>
          {d.what} — {d.why}
          {#if d.superseded_by}<span class="text-xs no-underline">
              → superseded by {d.superseded_by}</span
            >{/if}
        </li>
      {/each}
    </ol>
  {:else}
    <p class="text-xs text-muted">None logged.</p>
  {/each}
  <h3 class="mt-3 text-sm font-medium">Verification</h3>
  <ul class="text-xs">
    {#each t.verifications as v (v.verif_id)}
      <li class={v.state === 'current' ? '' : 'text-muted'}>
        L{v.level} by {v.by}{v.rerun ? ` (re-run ${v.rerun})` : ''} at
        <span class="font-mono">{v.sha.slice(0, 8)}</span> · {hhmm(v.ts)} · {v.state} — {v.evidence}
      </li>
    {:else}
      <li class="text-muted">Not verified yet.</li>
    {/each}
  </ul>
{:else}
  <p class="text-muted">Loading…</p>
{/if}
