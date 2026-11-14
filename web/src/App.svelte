<script lang="ts">
  // The dashboard (spec §10.1–10.2): the glance first and alone, drill-downs on click.
  // It polls the cursor every 500 ms and refetches only when it moves.
  import { onMount } from 'svelte'
  import { ApiError, Poller, makeClient, takeToken, type ConnState } from '$lib/api'
  import { applyBadge } from '$lib/badge'
  import Header from '$lib/glance/Header.svelte'
  import NeedsYouQueue from '$lib/glance/NeedsYou.svelte'
  import ProjectRowView from '$lib/glance/ProjectRow.svelte'
  import BranchGroups from '$lib/glance/BranchGroups.svelte'
  import SessionRowView from '$lib/glance/SessionRow.svelte'
  import ProjectView from '$lib/drill/Project.svelte'
  import SessionView from '$lib/drill/Session.svelte'
  import TaskView from '$lib/drill/Task.svelte'
  import HistoryView from '$lib/drill/History.svelte'
  import TuningView from '$lib/drill/Tuning.svelte'
  import CalibrationView from '$lib/drill/Calibration.svelte'
  import PagesView from '$lib/drill/Pages.svelte'
  import HiddenDrawer from '$lib/glance/HiddenDrawer.svelte'
  import { makeNav, type Screen } from '$lib/route'
  import type { Glance, OwnerAction } from '$lib/types'

  const MAX_PROJECTS = 5
  const MAX_SESSIONS = 8

  const token = takeToken()
  const client = makeClient(token)
  // Every screen change is a history entry, so Back and the swipe come back to the screen
  // before (spec 2026-11-14 §5).
  const nav = makeNav()
  let glance = $state<Glance | null>(null)
  let conn = $state<ConnState>({ connected: true, unauthorized: false, lastOk: null })
  let version = $state(0)
  let screen = $state<Screen>(nav.current())
  let showHidden = $state(false)
  /** Open a screen: one history entry, never two for the same screen. */
  function go(next: Screen) {
    nav.go(next)
    screen = next
  }
  let showAll = $state(false)
  let allProjects = $state(false)
  let allSessions = $state(false)

  const sessions = $derived(glance?.projects.flatMap((p) => p.sessions) ?? [])
  const shownSessions = $derived(allSessions ? sessions : sessions.slice(0, MAX_SESSIONS))
  const shownProjects = $derived(
    allProjects ? (glance?.projects ?? []) : (glance?.projects ?? []).slice(0, MAX_PROJECTS),
  )

  /** The page asked for is gone (a removed project, a bad link): said in words. */
  const gone = $derived(
    screen.kind === 'project' &&
      glance !== null &&
      !glance.projects.some((p) => p.repo_id === (screen as { repo: string }).repo) &&
      !(glance.hidden ?? []).some((h) => h.repo_id === (screen as { repo: string }).repo),
  )
  let goneNote = $state(false)
  $effect(() => {
    if (gone) {
      nav.replaceWithGlance()
      screen = { kind: 'glance' }
      goneNote = true
    }
  })

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

  onMount(() => nav.listen((s) => (screen = s)))

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

<Header
  {glance}
  {conn}
  {version}
  onShowAll={() => (showAll = true)}
  onShowHidden={() => (showHidden = true)}
/>
<HiddenDrawer
  items={glance?.hidden ?? []}
  open={showHidden && (glance?.hidden?.length ?? 0) > 0}
  onClose={() => (showHidden = false)}
  {act}
/>
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
        onclick={() => {
          if (!nav.toGlance()) screen = { kind: 'glance' }
        }}>← Glance</button
      >
    </nav>
    <div class="flex flex-col gap-8">
      {#if screen.kind === 'project'}
        <ProjectView
          {client}
          repo={screen.repo}
          {version}
          row={projectRow}
          onOpenTask={(repo, id) => go({ kind: 'task', repo, id })}
          onOpenHistory={(repo) => go({ kind: 'history', repo })}
          onOpenTuning={(repo) => go({ kind: 'tuning', repo })}
          onOpenCalibration={(repo) => go({ kind: 'calibration', repo })}
          onOpenPages={(repo) => go({ kind: 'pages', repo })}
        />
      {:else if screen.kind === 'session'}
        <SessionView {client} id={screen.id} {version} />
      {:else if screen.kind === 'task'}
        <TaskView {client} repo={screen.repo} id={screen.id} {version} />
      {:else if screen.kind === 'history'}
        <HistoryView {client} repo={screen.repo} />
      {:else if screen.kind === 'tuning'}
        <TuningView {client} repo={screen.repo} {version} {act} />
      {:else if screen.kind === 'calibration'}
        <CalibrationView {client} repo={screen.repo} {version} />
      {:else if screen.kind === 'pages'}
        <PagesView {client} repo={screen.repo} {version} />
      {/if}
    </div>
  {:else if glance}
    {#if goneNote}
      <p
        class="flex items-center gap-3 rounded-xl border border-line bg-surface p-4 text-row"
        role="status"
        data-testid="page-gone"
      >
        That page is gone.
        <button
          type="button"
          class="rounded-lg px-2 py-1 text-sm font-medium text-muted hover:bg-hover hover:text-fg"
          onclick={() => (goneNote = false)}>OK</button
        >
      </p>
    {/if}
    <NeedsYouQueue
      items={glance.needs_you}
      {act}
      disabled={!conn.connected}
      bind:showAll
      onOpenSession={(id) => go({ kind: 'session', id })}
    />
    <div class="grid grid-cols-1 gap-8 xl:grid-cols-[minmax(0,1fr)_400px]">
      <section aria-label="Projects" class="flex flex-col gap-3">
        <h2 class="text-base font-semibold">Projects</h2>
        <ul class="flex flex-col gap-4">
          {#each shownProjects as p (p.repo_id)}
            <ProjectRowView project={p} {act} onOpen={(repo) => go({ kind: 'project', repo })} />
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
        <BranchGroups groups={glance.branch_groups ?? []} />
      </section>
      <section aria-label="Live sessions" class="order-first flex flex-col gap-3 lg:order-none">
        <h2 class="flex items-baseline gap-2 text-base font-semibold">
          Sessions
          <span class="text-meta font-normal text-muted tabular-nums">{sessions.length}</span>
        </h2>
        <div class="rounded-xl border border-line bg-surface p-2">
          <ul class="flex flex-col gap-1">
            {#each shownSessions as s (s.session_id)}
              <SessionRowView session={s} onOpen={(id) => go({ kind: 'session', id })} />
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
