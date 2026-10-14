<script lang="ts">
  // Drift tuning (spec §7.6): the thresholds in force against the defaults, every change,
  // and a one-click revert (reversible: no confirm).
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
  <h2 class="text-lg font-medium">Drift tuning</h2>
  <p class="text-sm">
    lower {t.current.drift_lower} (default {t.defaults.drift_lower}) · upper {t.current.drift_upper}
    (default {t.defaults.drift_upper}) · tolerance {t.current.tolerance_pct}% (default
    {t.defaults.tolerance_pct}%) · {t.samples} samples
  </p>
  {#if t.revert}
    <div class="mt-2">
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
    </div>
  {/if}
  {#if message}<p class="text-meta" role="status">{message}</p>{/if}
  <ul class="mt-3 text-meta">
    {#each t.changes as c (c.ts + c.key)}
      <li>{hhmm(c.ts)} {c.key}: {c.old ?? '—'} → {c.new} ({c.samples} samples)</li>
    {:else}
      <li class="text-muted">No changes: the defaults are in force.</li>
    {/each}
  </ul>
{:else if m.error}
  <p class="text-warn">{m.error}</p>
{/if}
