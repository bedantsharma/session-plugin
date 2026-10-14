<script lang="ts">
  // Task drill-down (spec §10.2, plan §2): the title, details as a definition list,
  // decisions with superseded-by chains, and verification evidence.
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
  <p class="text-row text-warn">{m.error}</p>
{:else if m.data}
  {@const t = m.data}
  <header class="flex flex-col gap-4">
    <h2 class="text-xl font-semibold">
      <span class="font-mono text-muted">{t.id}</span>
      {t.title}
    </h2>
    <dl
      class="grid max-w-2xl grid-cols-[10rem_minmax(0,1fr)] gap-x-6 gap-y-2 rounded-xl border border-line bg-surface p-5"
    >
      <dt class="text-meta text-muted">Status</dt>
      <dd class="text-row">{t.status ?? '—'}</dd>
      <dt class="text-meta text-muted">Time</dt>
      <dd class="text-row tabular-nums">{t.actual_min} of {t.estimate_min ?? '?'} min</dd>
      <dt class="text-meta text-muted">Retries</dt>
      <dd class="text-row tabular-nums">{t.retries}</dd>
      {#if t.actors.length}
        <dt class="text-meta text-muted">Worked on by</dt>
        <dd class="text-row">
          {t.actors.map((a) => [a.agent_type, a.model].filter(Boolean).join(' ')).join(', ')}
        </dd>
      {/if}
    </dl>
  </header>

  <section class="flex flex-col gap-3">
    <h3 class="text-base font-semibold">Decisions</h3>
    {#each t.decisions as chain (chain[0].decision_id)}
      <ol class="flex max-w-[80ch] flex-col divide-y divide-line border-y border-line">
        {#each chain as d (d.decision_id)}
          <li
            class={['flex flex-col gap-0.5 py-2.5', d.superseded_by && 'text-muted']}
            data-decision={d.decision_id}
          >
            <span class={['text-row', d.superseded_by && 'line-through']}>{d.what}</span>
            <span class="text-meta text-muted">
              <span class="font-mono">{d.decision_id}</span> · {d.why}
              {#if d.superseded_by}· superseded by <span class="font-mono">{d.superseded_by}</span
                >{/if}
            </span>
          </li>
        {/each}
      </ol>
    {:else}
      <p class="text-row text-muted">None logged.</p>
    {/each}
  </section>

  <section class="flex flex-col gap-3">
    <h3 class="text-base font-semibold">Verification</h3>
    <ul class="flex flex-col divide-y divide-line border-y border-line">
      {#each t.verifications as v (v.verif_id)}
        <li
          class={[
            'flex min-h-11 flex-col justify-center gap-0.5 py-2',
            v.state !== 'current' && 'text-muted',
          ]}
        >
          <span class="text-row">
            L{v.level} by {v.by}{v.rerun ? ` (re-run ${v.rerun})` : ''} · {v.state}
          </span>
          <span class="text-meta text-muted">
            <span class="font-mono">{v.sha.slice(0, 8)}</span> · {hhmm(v.ts)} · {v.evidence}
          </span>
        </li>
      {:else}
        <li class="py-3 text-row text-muted">Not verified yet.</li>
      {/each}
    </ul>
  </section>
{:else}
  <p class="text-row text-muted">Loading…</p>
{/if}
