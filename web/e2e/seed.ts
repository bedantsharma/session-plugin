// Seeds a temp bridge-screen home through the event log: events are inserted, then
// `bridge-screen rebuild` replays them into the views exactly as the hooks would have.
import { execFileSync } from 'node:child_process'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

export interface Seed {
  repo: string
  name: string
  sessions: { id: string; red?: boolean }[]
}

let n = 0
const id = () => `01SEED${String(++n).padStart(20, '0')}`

export function seed(bin: string, home: string, repos: Seed[]) {
  const env = { ...process.env, BRIDGE_SCREEN_HOME: home }
  execFileSync(bin, ['install', '--no-plugin'], { env })
  const db = new DatabaseSync(join(home, 'bridge-screen.db'))
  const now = Date.now()
  const ago = (min: number) => new Date(now - min * 60_000).toISOString()
  const seq = new Map<string, number>()
  const add = (repo: string, session: string, ts: string, kind: string, data: object) => {
    const s = (seq.get(session) ?? 0) + 1
    seq.set(session, s)
    db.prepare(
      `INSERT INTO events (id, v, ts, repo_id, session_id, seq, kind, actor, task_id, data)
       VALUES (?, 1, ?, ?, ?, ?, ?, '{}', NULL, ?)`,
    ).run(id(), ts, repo, session, s, kind, JSON.stringify(data))
  }
  for (const r of repos) {
    db.prepare(`INSERT INTO repos (id, name, root, created_at) VALUES (?, ?, ?, ?)`).run(
      r.repo,
      r.name,
      `/code/${r.name}`,
      ago(60),
    )
    add(r.repo, 'owner', ago(59), 'node_added', {
      id: 'G',
      parent_id: null,
      kind: 'goal',
      title: `Ship ${r.name}`,
      intent: 'People can use it.',
      estimate_min: null,
      expected_files: [],
    })
    add(r.repo, 'owner', ago(59), 'node_added', {
      id: '1',
      parent_id: 'G',
      kind: 'task',
      title: 'Login form',
      estimate_min: 30,
      expected_files: [],
    })
    for (const s of r.sessions) {
      add(r.repo, s.id, ago(50), 'session_started', {
        cwd: `/wt/${s.id}`,
        source: 'startup',
        worktree: `/wt/${s.id}`,
        branch: 'feat',
      })
      add(r.repo, s.id, ago(45), 'prompt', { chars: 5 })
      if (s.red) {
        add(r.repo, s.id, ago(45), 'detour_opened', {
          detour_id: `D-${s.id}`,
          reason: 'CI cache is broken',
          resume_cue: 'rerun the login test',
        })
      }
      add(r.repo, s.id, ago(5), 'unit_stopped', {})
      if (s.red) {
        add(r.repo, s.id, ago(5), 'unit_labeled', {
          unit_no: 1,
          label: 'detour',
          source: 'declared',
        })
        add(r.repo, s.id, ago(5), 'nav_state_changed', {
          from: 'green',
          to: 'red',
          away_secs: 2400,
        })
      }
    }
  }
  db.close()
  execFileSync(bin, ['rebuild'], { env })
}
