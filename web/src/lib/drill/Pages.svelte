<script lang="ts">
  // Project pages: what the project is for, how it fits together, where things stand, why
  // choices were made, and the words used — shown as plain text, read-only. They live in
  // bridge-screen's own folder, never in the repo; agents keep them current.
  import type { Client } from '$lib/api'
  import type { PagesView } from '$lib/types'
  import { useModel } from './useModel.svelte'

  let { client, repo, version }: { client: Client; repo: string; version: number } = $props()

  const m = useModel<PagesView>(
    () => client,
    () => `/api/pages/${encodeURIComponent(repo)}`,
    () => version,
  )
</script>

<h2 class="text-xl font-semibold">Project pages</h2>
{#if m.error}
  <p class="text-meta text-muted">Couldn’t load the project pages: {m.error}</p>
{:else if m.data}
  {@const v = m.data}
  {#if v.state === 'not_set_up'}
    <p class="max-w-3xl text-row">
      Project pages aren’t set up for this project yet. To create them, run
      <code class="font-mono">bridge-screen pages set-up</code> in the project’s folder.
    </p>
  {:else if v.state === 'declined'}
    <p class="max-w-3xl text-row">
      You chose not to have project pages for this project. To change your mind, run
      <code class="font-mono">bridge-screen pages set-up</code> in the project’s folder.
    </p>
  {:else}
    <p class="max-w-3xl text-meta text-muted">
      Read-only. Your agents keep these current; they are kept outside the project’s files.
    </p>
    <div class="flex max-w-3xl flex-col gap-4">
      {#each v.pages as p (p.title)}
        <section class="flex flex-col gap-2 rounded-xl border border-line bg-surface p-5">
          <div class="flex flex-wrap items-baseline justify-between gap-2">
            <h3 class="text-row font-semibold">{p.title}</h3>
            <span class="text-meta text-muted tabular-nums">
              {p.updated ? `Updated ${p.updated}` : 'Not written yet'}
            </span>
          </div>
          <pre class="font-sans text-row break-words whitespace-pre-wrap">{p.text}</pre>
        </section>
      {/each}
    </div>
  {/if}
{/if}
