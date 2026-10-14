<script lang="ts">
  // The dashboard (spec §10.1–10.2): the glance first and alone, drill-downs on click.
  // It polls the cursor every 500 ms and refetches only when it moves.
  import { onMount } from 'svelte'
  import { ApiError, Poller, makeClient, takeToken, type ConnState } from '$lib/api'
  import { applyBadge } from '$lib/badge'
  import Header from '$lib/glance/Header.svelte'
  import NeedsYouQueue from '$lib/glance/NeedsYou.svelte'
  import ProjectRowView from '$lib/glance/ProjectRow.svelte'
  import SessionRowView from '$lib/glance/SessionRow.svelte'
  import ProjectView from '$lib/drill/Project.svelte'
  import SessionView from '$lib/drill/Session.svelte'
  import TaskView from '$lib/drill/Task.svelte'
  import HistoryView from '$lib/drill/History.svelte'
  import TuningView from '$lib/drill/Tuning.svelte'
  import type { Glance, OwnerAction } from '$lib/types'

  type Screen =
    | { kind: 'glance' }
    | { kind: 'project'; repo: string }
    | { kind: 'session'; id: string }
    | { kind: 'task'; repo: string; id: string }
    | { kind: 'history'; repo: string }
    | { kind: 'tuning'; repo: string }

  const MAX_PROJECTS = 5
  const MAX_SESSIONS = 8

  const token = takeToken()
  const client = makeClient(token)
  let glance = $state<Glance | null>(null)
  let conn = $state<ConnState>({ connected: true, unauthorized: false, lastOk: null })
  let version = $state(0)
  let screen = $state<Screen>({ kind: 'glance' })
  let showAll = $state(false)
  let allProjects = $state(false)
  let allSessions = $state(false)

  const sessions = $derived(glance?.projects.flatMap((p) => p.sessions) ?? [])
  const shownSessions = $derived(allSessions ? sessions : sessions.slice(0, MAX_SESSIONS))
  const shownProjects = $derived(
    allProjects ? (glance?.projects ?? []) : (glance?.projects ?? []).slice(0, MAX_PROJECTS),
  )

  $effect(() => applyBadge(glance?.needs_you_count ?? 0, conn.connected && glance !== null))

  async function act(action: OwnerAction) {
    const res = await client.act(action)
    poller.wake()
    return res
  }

  const poller = new Poller({
    version: async () =>
      (await client.get<{ last_event_pos: number }>('/api/version')).last_event_pos,
    onChange: async (pos) => {
      glance = await client.get<Glance>('/api/glance')
      version = pos
    },
    onState: (s) => (conn = s),
  })

  onMount(() => {
    if (!token) {
      conn = { connected: false, unauthorized: true, lastOk: null }
      return
    }
    poller.start()
    const vis = () => poller.wake()
    document.addEventListener('visibilitychange', vis)
    return () => {
      poller.stop()
      document.removeEventListener('visibilitychange', vis)
    }
  })

  // Live times (minutes ago) move without new events: refresh the glance now and then.
  onMount(() => {
    const t = setInterval(() => {
      if (conn.connected && token && !document.hidden)
        client
          .get<Glance>('/api/glance')
          .then((g) => (glance = g))
          .catch((e) => e instanceof ApiError || null)
    }, 30_000)
    return () => clearInterval(t)
  })
</script>

<main class="mx-auto flex h-dvh max-w-6xl flex-col gap-3 overflow-hidden p-4">
  <Header {glance} {conn} onShowAll={() => (showAll = true)} />
  {#if conn.unauthorized}
    <section class="rounded-lg border border-warn p-4 text-sm" role="alert">
      <p class="font-medium text-warn">Token missing or refused.</p>
      <p class="mt-1">
        Open the URL that <code class="font-mono">bridge-screen ui</code> printed (it carries the
        token after <code class="font-mono">#token=</code>). Running it again prints the URL of the
        dashboard that is already running.
      </p>
    </section>
  {:else if screen.kind !== 'glance'}
    <nav>
      <button
        type="button"
        class="text-sm text-muted hover:underline"
        onclick={() => (screen = { kind: 'glance' })}>← glance</button
      >
    </nav>
    <div class="min-h-0 flex-1 overflow-auto">
      {#if screen.kind === 'project'}
        <ProjectView
          {client}
          repo={screen.repo}
          {version}
          onOpenTask={(repo, id) => (screen = { kind: 'task', repo, id })}
          onOpenHistory={(repo) => (screen = { kind: 'history', repo })}
          onOpenTuning={(repo) => (screen = { kind: 'tuning', repo })}
        />
      {:else if screen.kind === 'session'}
        <SessionView {client} id={screen.id} {version} />
      {:else if screen.kind === 'task'}
        <TaskView {client} repo={screen.repo} id={screen.id} {version} />
      {:else if screen.kind === 'history'}
        <HistoryView {client} repo={screen.repo} />
      {:else if screen.kind === 'tuning'}
        <TuningView {client} repo={screen.repo} {version} {act} />
      {/if}
    </div>
  {:else if glance}
    <NeedsYouQueue
      items={glance.needs_you}
      {act}
      disabled={!conn.connected}
      bind:showAll
      onOpenSession={(id) => (screen = { kind: 'session', id })}
    />
    <section aria-label="Projects">
      <ul class="flex flex-col">
        {#each shownProjects as p (p.repo_id)}
          <ProjectRowView project={p} onOpen={(repo) => (screen = { kind: 'project', repo })} />
        {/each}
      </ul>
      {#if glance.projects.length > MAX_PROJECTS && !allProjects}
        <button
          type="button"
          class="px-2 text-xs text-muted hover:underline"
          onclick={() => (allProjects = true)}>{glance.projects.length - MAX_PROJECTS} more</button
        >
      {/if}
    </section>
    <section aria-label="Live sessions" class="border-t border-line pt-2">
      <ul class="flex flex-col">
        {#each shownSessions as s (s.session_id)}
          <SessionRowView session={s} onOpen={(id) => (screen = { kind: 'session', id })} />
        {/each}
      </ul>
      {#if sessions.length > MAX_SESSIONS && !allSessions}
        <button
          type="button"
          class="px-2 text-xs text-muted hover:underline"
          onclick={() => (allSessions = true)}>{sessions.length - MAX_SESSIONS} more</button
        >
      {/if}
      {#if sessions.length === 0}
        <p class="px-2 text-sm text-muted">No live sessions.</p>
      {/if}
    </section>
  {/if}
</main>
