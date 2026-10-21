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

  const projectRow = $derived(
    screen.kind === 'project'
      ? glance?.projects.find((p) => p.repo_id === (screen as { repo: string }).repo)
      : undefined,
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

<Header {glance} {conn} {version} onShowAll={() => (showAll = true)} />
<main class="mx-auto flex w-full max-w-[1280px] flex-col gap-8 px-4 py-8 lg:px-6 xl:px-8">
  {#if conn.unauthorized}
    <section class="rounded-xl border border-warn bg-surface p-5 text-row" role="alert">
      <p class="font-medium text-warn">Token missing or refused.</p>
      <p class="mt-1">
        Open the URL that <code class="font-mono">bridge-screen ui</code> printed (it carries the
        token after <code class="font-mono">#token=</code>). Running it again prints the URL of the
        dashboard that is already running.
      </p>
    </section>
  {:else if screen.kind !== 'glance'}
    <nav class="-mt-4 -mb-4">
      <button
        type="button"
        class="-ml-2 inline-flex h-9 items-center rounded-lg px-2 text-sm font-medium text-muted hover:bg-hover hover:text-fg"
        onclick={() => (screen = { kind: 'glance' })}>← Glance</button
      >
    </nav>
    <div class="flex flex-col gap-8">
      {#if screen.kind === 'project'}
        <ProjectView
          {client}
          repo={screen.repo}
          {version}
          row={projectRow}
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
    <div class="grid grid-cols-1 gap-8 xl:grid-cols-[minmax(0,1fr)_400px]">
      <section aria-label="Projects" class="flex flex-col gap-3">
        <h2 class="text-base font-semibold">Projects</h2>
        <ul class="flex flex-col gap-4">
          {#each shownProjects as p (p.repo_id)}
            <ProjectRowView project={p} onOpen={(repo) => (screen = { kind: 'project', repo })} />
          {/each}
        </ul>
        {#if glance.projects.length === 0}
          <p class="text-row text-muted">
            No projects yet. Start a Claude Code session in a repository to track it here.
          </p>
        {/if}
        {#if glance.projects.length > MAX_PROJECTS && !allProjects}
          <button
            type="button"
            class="self-start rounded-lg px-2 py-1 text-sm font-medium text-muted hover:bg-hover hover:text-fg"
            onclick={() => (allProjects = true)}
            >{glance.projects.length - MAX_PROJECTS} more</button
          >
        {/if}
      </section>
      <section aria-label="Live sessions" class="order-first flex flex-col gap-3 lg:order-none">
        <h2 class="flex items-baseline gap-2 text-base font-semibold">
          Sessions
          <span class="text-meta font-normal text-muted tabular-nums">{sessions.length}</span>
        </h2>
        <div class="rounded-xl border border-line bg-surface p-2">
          <ul class="flex flex-col gap-1">
            {#each shownSessions as s (s.session_id)}
              <SessionRowView session={s} onOpen={(id) => (screen = { kind: 'session', id })} />
            {/each}
          </ul>
          {#if sessions.length > MAX_SESSIONS && !allSessions}
            <button
              type="button"
              class="mt-1 rounded-lg px-3 py-1 text-sm font-medium text-muted hover:bg-hover hover:text-fg"
              onclick={() => (allSessions = true)}>{sessions.length - MAX_SESSIONS} more</button
            >
          {/if}
          {#if sessions.length === 0}
            <p class="px-3 py-3 text-row text-muted">No live sessions.</p>
          {/if}
        </div>
      </section>
    </div>
  {/if}
</main>
