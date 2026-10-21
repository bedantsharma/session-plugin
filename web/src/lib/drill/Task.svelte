<script lang="ts">
  // Task drill-down (spec §9.2, §10.2, plan §2): the title, details as a definition list
  // (status, estimate vs actual and the ratio once done, retries, steps, who worked on
  // it), tokens and cost by model and by agent, decisions with superseded-by chains, and
  // verification evidence.
  import type { Client } from '$lib/api'
  import { hhmm } from '$lib/format'
  import type { TaskDrill } from '$lib/types'
  import ActionButton from '$lib/glance/ActionButton.svelte'
  import { statusWord, verificationWord } from '$lib/words'
  import { useModel } from './useModel.svelte'

  let { client, repo, id, version }: { client: Client; repo: string; id: string; version: number } =
    $props()

  const m = useModel<TaskDrill>(
    () => client,
    () => `/api/task/${encodeURIComponent(repo)}/${encodeURIComponent(id)}`,
    () => version,
  )

  let pending = $state(false)
  let reply = $state<{ ok: boolean; text: string } | null>(null)

  async function takeBack(action: TaskDrill['withdraw']) {
    if (!action || pending) return
    pending = true
    const res = await client.act(action.action)
    pending = false
    reply = res.ok ? { ok: true, text: res.reply } : { ok: false, text: res.error }
  }
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
      <dd class="text-row">{t.status ? statusWord(t.status).word : '—'}</dd>
      <dt class="text-meta text-muted">Time</dt>
      <dd class="text-row tabular-nums" data-testid="task-time">
        {t.actual_min} of {t.estimate_min ?? '?'} min{#if t.ratio !== null}<span class="text-muted"
            >{` · took ${Number(t.ratio.toFixed(1))}× its estimate`}</span
          >{/if}
      </dd>
      <dt class="text-meta text-muted">Retries</dt>
      <dd class="text-row tabular-nums" title="Units of work after a failed verification">
        {t.retries}
      </dd>
      {#if t.steps.length}
        <dt class="text-meta text-muted">Steps</dt>
        <dd class="text-row" data-testid="steps">
          <ol class="flex flex-col gap-0.5">
            {#each t.steps as st (st.id)}
              <li class={[st.current ? 'font-medium' : 'text-muted']} data-step={st.id}>
                <span class="font-mono">{st.id}</span>
                {st.title} · {statusWord(st.status).word}{#if st.current}<span class="text-clay"
                    >&nbsp;· current</span
                  >{/if}
              </li>
            {/each}
          </ol>
        </dd>
      {/if}
      <dt class="text-meta text-muted">Cost</dt>
      <dd class="text-row tabular-nums" data-testid="task-cost">
        {t.usage ? t.usage.total.summary : 'no tokens recorded yet'}
      </dd>
      {#if t.actors.length}
        <dt class="text-meta text-muted">Worked on by</dt>
        <dd class="text-row">
          {t.actors.map((a) => [a.agent_type, a.model].filter(Boolean).join(' ')).join(', ')}
        </dd>
      {/if}
    </dl>
  </header>

  {#if t.usage}
    <section class="flex flex-col gap-3" aria-labelledby="tokens">
      <h3 id="tokens" class="text-base font-semibold">Tokens and cost</h3>
      <div class="grid max-w-3xl grid-cols-1 gap-6 md:grid-cols-2">
        {#each [{ name: 'By model', rows: t.usage.by_model }, { name: 'By agent', rows: t.usage.by_agent }] as part (part.name)}
          <table class="w-full text-row tabular-nums" aria-label={part.name}>
            <thead class="text-meta text-muted">
              <tr>
                <th class="py-1 text-left font-normal">{part.name}</th>
                <th class="text-right font-normal">Tokens</th>
                <th class="text-right font-normal">Cost</th>
              </tr>
            </thead>
            <tbody class="divide-y divide-line border-y border-line">
              {#each part.rows as r (r.label)}
                <tr>
                  <td class="py-1.5">{r.label}</td>
                  <td class="text-right">{r.tokens_text}</td>
                  <td class={['text-right', r.cost_text === 'cost unknown' && 'text-warn']}
                    >{r.cost_text}</td
                  >
                </tr>
              {/each}
            </tbody>
          </table>
        {/each}
      </div>
    </section>
  {/if}

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
    <div class="flex flex-wrap items-center gap-3">
      <h3 class="text-base font-semibold">Verification</h3>
      {#if t.withdraw}
        <ActionButton
          label={t.withdraw.label}
          tone={t.withdraw.tone}
          compact
          {pending}
          onclick={() => takeBack(t.withdraw)}
        />
      {/if}
    </div>
    {#if reply}
      <p class={['text-meta', reply.ok ? 'text-muted' : 'text-warn']} role="status">
        {reply.text}
      </p>
    {/if}
    <ul class="flex flex-col divide-y divide-line border-y border-line">
      {#each t.verifications as v (v.verif_id)}
        <li
          class={[
            'flex min-h-11 flex-col justify-center gap-0.5 py-2',
            v.state !== 'current' && 'text-muted',
          ]}
        >
          <span class="text-row">
            {verificationWord(v)}
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
