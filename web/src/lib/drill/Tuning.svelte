<script lang="ts">
  // Drift tuning (spec §7.6, plan §2): the thresholds in force against the defaults, every
  // change, and a one-click revert (reversible: no confirm).
  import ActionButton from '$lib/glance/ActionButton.svelte'
  import type { ActResult, Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { OwnerAction, TuningDrill } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let {
    client,
    repo,
    version,
    act,
  }: {
    client: Client
    repo: string
    version: number
    act: (a: OwnerAction) => Promise<ActResult>
  } = $props()

  const m = useModel<TuningDrill>(
    () => client,
    () => `/api/tuning/${encodeURIComponent(repo)}`,
    () => version,
  )
  let pending = $state(false)
  let message = $state<string | null>(null)
</script>

{#if m.data}
  {@const t = m.data}
  {@const rows = [
    {
      name: 'Lower threshold',
      what: 'Drift scores below this count as on task.',
      now: t.current.drift_lower,
      def: t.defaults.drift_lower,
    },
    {
      name: 'Upper threshold',
      what: 'Scores above this count as drift; in between, the agent is asked.',
      now: t.current.drift_upper,
      def: t.defaults.drift_upper,
    },
    {
      name: 'Tolerance',
      what: 'Detour time allowed, as a share of the task’s estimate.',
      now: `${t.current.tolerance_pct}%`,
      def: `${t.defaults.tolerance_pct}%`,
    },
  ]}
  <h2 class="text-xl font-semibold">Drift tuning</h2>
  <section class="flex max-w-3xl flex-col gap-4 rounded-xl border border-line bg-surface p-5">
    <ul class="flex flex-col divide-y divide-line">
      {#each rows as r (r.name)}
        <li class="flex min-h-11 items-center gap-4 py-2">
          <div class="flex min-w-0 flex-1 flex-col gap-0.5">
            <span class="text-row">{r.name}</span>
            <span class="text-meta text-muted">{r.what}</span>
          </div>
          <span class="w-16 text-right text-row font-medium tabular-nums">{r.now}</span>
          <span class="w-28 text-right text-meta text-muted tabular-nums">default {r.def}</span>
        </li>
      {/each}
    </ul>
    <div class="flex flex-wrap items-center gap-3 border-t border-line pt-4">
      <span class="text-meta text-muted tabular-nums">{t.samples} samples</span>
      {#if t.revert}
        <span class="ml-auto">
          <ActionButton
            label={t.revert.label}
            tone={t.revert.tone}
            {pending}
            disabled={pending}
            onclick={async () => {
              pending = true
              const r = await act(t.revert!.action)
              pending = false
              message = r.ok ? r.reply : r.error
            }}
          />
        </span>
      {/if}
    </div>
    {#if message}<p class="text-meta" role="status">{message}</p>{/if}
  </section>
  <section class="flex flex-col gap-3">
    <h3 class="text-base font-semibold">Changes</h3>
    <ul class="flex max-w-3xl flex-col divide-y divide-line border-y border-line">
      {#each t.changes as c (c.ts + c.key)}
        <li class="flex h-9 items-center gap-3 text-row">
          <span class="w-14 text-meta text-muted tabular-nums">{hhmm(c.ts)}</span>
          <span class="font-mono text-meta">{c.key}</span>
          <span class="tabular-nums">{c.old ?? '—'} → {c.new}</span>
          <span class="ml-auto text-meta text-muted tabular-nums">{c.samples} samples</span>
        </li>
      {:else}
        <li class="py-3 text-row text-muted">No changes: the defaults are in force.</li>
      {/each}
    </ul>
  </section>
{:else if m.error}
  <p class="text-row text-warn">{m.error}</p>
{/if}
