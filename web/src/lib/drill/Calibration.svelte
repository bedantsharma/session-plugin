<script lang="ts">
  // Calibration drill-down (spec §9.3, §10.2): what the forecast rests on. Per model, how
  // long verified tasks took against their estimates (a dot per task); the forecast's
  // history as a P50/P90 band over time (its convergence); and the prices in use, with
  // unpriced models flagged. Below the minimum of finished tasks there are no dates, only
  // the reason.
  import { LineChart, ScatterChart } from 'layerchart'
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { CalibrationDrill } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let { client, repo, version }: { client: Client; repo: string; version: number } = $props()

  const m = useModel<CalibrationDrill>(
    () => client,
    () => `/api/calibration/${encodeURIComponent(repo)}`,
    () => version,
  )

  const dots = $derived(
    (m.data?.pooled ?? []).map((p) => ({
      date: new Date(p.completed_at),
      ratio: p.ratio,
      model: p.model,
    })),
  )
  const band = $derived(
    (m.data?.history ?? []).map((h) => ({
      date: new Date(h.ts),
      p50: h.p50_hours,
      p90: h.p90_hours,
    })),
  )
  // A chart needs two distinct times a minute or more apart; otherwise show the numbers.
  function chartable(times: number[]): boolean {
    if (new Set(times).size < 2) return false
    return Math.max(...times) - Math.min(...times) >= 60_000
  }
  const dotsChartable = $derived(chartable(dots.map((d) => d.date.getTime())))
  const bandChartable = $derived(chartable(band.map((d) => d.date.getTime())))

  /** `1.8×`, one decimal at most. */
  function times(r: number): string {
    return `${Number(r.toFixed(1))}×`
  }
  /** `$4.00`. */
  function usd(n: number): string {
    return `$${n.toFixed(2)}`
  }
  /** `4.5h` (one decimal at most). */
  function hours(h: number): string {
    return `${Number((Math.floor(h * 10) / 10).toFixed(1))}h`
  }
</script>

{#if m.error}
  <p class="text-row text-warn">{m.error}</p>
{:else if m.data}
  {@const c = m.data}
  <header class="flex flex-col gap-2">
    <h2 class="text-xl font-semibold">Calibration · {c.name}</h2>
    <p class="text-row" data-testid="calibration-forecast">
      {#if c.forecast.compact}
        <span title={c.forecast.help}>{c.forecast.text}</span>
        <span class="text-meta text-muted">({c.forecast.help})</span>
      {:else}
        <span class="text-muted">{c.forecast.text}</span>
      {/if}
    </p>
    {#if c.pooled.length}
      <p class="text-meta text-muted">{c.forecast.basis}</p>
    {/if}
    {#each c.notes as note (note)}
      <p class="text-meta text-warn" role="note">{note}</p>
    {/each}
  </header>

  <section class="flex flex-col gap-3" aria-labelledby="ratios">
    <h3 id="ratios" class="text-base font-semibold">How long tasks took against their estimates</h3>
    {#if c.models.length}
      <ul class="flex max-w-3xl flex-col divide-y divide-line border-y border-line">
        {#each c.models as row (row.model)}
          <li class="flex min-h-11 items-center gap-4 py-2" data-model={row.model}>
            <span class="w-32 text-row">{row.name}</span>
            <span class="min-w-0 flex-1 text-row">{row.text}</span>
            <span class="w-24 text-right text-meta text-muted tabular-nums"
              >{row.points.length} {row.points.length === 1 ? 'task' : 'tasks'}</span
            >
            <span class="w-40 text-right text-meta text-muted"
              >{row.own ? 'used on its own' : `pooled until ${c.model_points} tasks`}</span
            >
          </li>
        {/each}
      </ul>
      <div class="max-w-3xl rounded-xl border border-line bg-surface p-5">
        {#if dotsChartable}
          <div class="h-56" aria-label="Each finished task: actual time ÷ estimate, over time">
            <ScatterChart
              data={dots}
              x="date"
              y="ratio"
              series={[{ key: 'ratio', label: 'actual ÷ estimate', color: 'var(--clay)' }]}
              legend
            />
          </div>
        {:else}
          <ul class="flex flex-col gap-1 text-row tabular-nums" data-testid="ratio-list">
            {#each c.pooled as p (p.task_id)}
              <li>
                <span class="font-mono text-muted">{p.task_id}</span>
                {p.title} · {Math.round(p.actual_min)} of {p.estimate_min} min ({times(p.ratio)})
              </li>
            {/each}
          </ul>
          <p class="mt-2 text-meta text-muted">
            a chart appears once tasks finish at different times
          </p>
        {/if}
      </div>
    {:else}
      <p class="text-row text-muted" data-testid="no-points">
        No verified tasks with an estimate and recorded time yet. The forecast needs
        {c.min_points}.
      </p>
    {/if}
  </section>

  <section class="flex flex-col gap-3" aria-labelledby="convergence">
    <h3 id="convergence" class="text-base font-semibold">Forecast over time</h3>
    <div class="max-w-3xl rounded-xl border border-line bg-surface p-5">
      {#if bandChartable}
        <div class="h-56" aria-label="Agent hours left, 50% and 90% chance, over time">
          <LineChart
            data={band}
            x="date"
            series={[
              { key: 'p50', label: 'likely (h)', color: 'var(--clay)' },
              { key: 'p90', label: 'could take (h)', color: 'var(--muted)' },
            ]}
            legend
          />
        </div>
      {:else if c.history.length}
        <ul class="flex flex-col gap-1 text-row tabular-nums" data-testid="history-list">
          {#each c.history as h (h.ts)}
            <li>
              <span class="text-meta text-muted">{hhmm(h.ts)}</span>
              {h.reason ??
                `${hours(h.p50_hours)} likely · up to ${hours(h.p90_hours)} of agent time`}
            </li>
          {/each}
        </ul>
        <p class="mt-2 text-meta text-muted">a chart appears once there are two forecasts</p>
      {:else}
        <p class="text-row text-muted" data-testid="no-history">
          No forecast recorded yet: one is kept each time a task is verified.
        </p>
      {/if}
    </div>
  </section>

  <section class="flex flex-col gap-3" aria-labelledby="prices">
    <h3 id="prices" class="text-base font-semibold">Prices in use</h3>
    <p class="text-meta text-muted">USD per million tokens, from config.toml.</p>
    <table class="w-full max-w-3xl text-row tabular-nums">
      <thead class="text-meta text-muted">
        <tr>
          <th class="py-1 text-left font-normal">Model</th>
          <th class="text-right font-normal">Input</th>
          <th class="text-right font-normal">Output</th>
          <th class="text-right font-normal">Cache read</th>
          <th class="text-right font-normal">Cache write 5m</th>
          <th class="text-right font-normal">1h</th>
          <th class="text-right font-normal">From</th>
        </tr>
      </thead>
      <tbody class="divide-y divide-line border-y border-line">
        {#each c.unpriced_models as model (model)}
          <tr data-unpriced={model}>
            <td class="py-1.5 font-mono text-meta">{model}</td>
            <td colspan="6" class="text-right text-warn">no price — cost unknown</td>
          </tr>
        {/each}
        {#each c.prices as p (p.model)}
          <tr>
            <td class="py-1.5"
              >{p.name}{#if p.name !== p.model}<span class="ml-2 font-mono text-meta text-muted"
                  >{p.model}</span
                >{/if}</td
            >
            <td class="text-right">{usd(p.price.input)}</td>
            <td class="text-right">{usd(p.price.output)}</td>
            <td class="text-right">{usd(p.price.cache_read)}</td>
            <td class="text-right">{usd(p.price.cache_write_5m)}</td>
            <td class="text-right">{usd(p.price.cache_write_1h)}</td>
            <td class="text-right text-meta text-muted">{p.source}</td>
          </tr>
        {/each}
      </tbody>
    </table>
  </section>
{:else}
  <p class="text-row text-muted">Loading…</p>
{/if}
