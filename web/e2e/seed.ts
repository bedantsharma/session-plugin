// Seeds a temp bridge-screen home through the event log: events are inserted, then
// `bridge-screen rebuild` replays them into the views exactly as the hooks would have.
import { execFileSync } from 'node:child_process'
import { join } from 'node:path'
import { DatabaseSync } from 'node:sqlite'

export interface Seed {
  repo: string
  name: string
  sessions: { id: string; red?: boolean; working?: boolean; units?: number; task?: string }[]
  /** Milestones with hills under the goal (for the screenshot step). */
  milestones?: { id: string; title: string; hill?: 'uphill' | 'downhill' }[]
  /** Tasks already verified at L2, with time and tokens (the calibration drill). */
  finished?: number
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
    for (const m of r.milestones ?? []) {
      add(r.repo, 'owner', ago(59), 'node_added', {
        id: m.id,
        parent_id: 'G',
        kind: 'milestone',
        title: m.title,
        estimate_min: null,
        expected_files: [],
      })
      add(r.repo, 'owner', ago(59), 'node_added', {
        id: `${m.id}.1`,
        parent_id: m.id,
        kind: 'task',
        title: `First step of ${m.title.toLowerCase()}`,
        estimate_min: 45,
        expected_files: [],
      })
      if (m.hill) add(r.repo, 'owner', ago(58), 'hill_moved', { id: m.id, hill: m.hill })
    }
    // Finished tasks: worked on, with replies on two models, then verified at L2.
    for (let i = 0; i < (r.finished ?? 0); i++) {
      const task = `9.${i + 1}`
      const sid = `done${i}`
      const start = 59 - i * 4
      const took = 3 + (i % 3)
      if (i === 0)
        add(r.repo, 'owner', ago(59), 'node_added', {
          id: '9',
          parent_id: 'G',
          kind: 'milestone',
          title: 'Done already',
          estimate_min: null,
          expected_files: [],
        })
      add(r.repo, 'owner', ago(59), 'node_added', {
        id: task,
        parent_id: '9',
        kind: 'task',
        title: `Finished step ${i + 1}`,
        estimate_min: 2,
        expected_files: [],
      })
      add(r.repo, sid, ago(start), 'session_started', { cwd: '/wt/done', source: 'startup' })
      add(r.repo, sid, ago(start), 'task_claimed', { task_id: task, mode: 'auto', confidence: 1 })
      add(r.repo, sid, ago(start), 'prompt', { chars: 5 })
      add(r.repo, sid, ago(start - 1), 'usage', {
        path: `/t/${sid}.jsonl`,
        offset_after: 10,
        replies: [
          {
            message_id: `${sid}-a`,
            model: 'claude-opus-5-5',
            ts: ago(start - 1),
            input: 40_000,
            output: 9_000,
            cache_read: 800_000,
          },
          {
            message_id: `${sid}-b`,
            model: 'claude-haiku-4-5',
            ts: ago(start - 1),
            input: 5_000,
            output: 2_000,
          },
        ],
      })
      add(r.repo, sid, ago(start - took), 'unit_stopped', {})
      add(r.repo, sid, ago(start - took), 'verified', {
        verif_id: `v-${sid}`,
        task_id: task,
        level: 2,
        sha: 'abc1234',
        files: [],
        evidence: 'tests pass',
        by: 'agent',
      })
      add(r.repo, 'owner', ago(start - took), 'forecast_ran', {
        p50_hours: 1.5 - i * 0.1,
        p90_hours: 2.5 - i * 0.2,
        p50_date: null,
        p90_date: null,
        remaining_min: 120,
        unestimated: 0,
        points: i + 1,
        hours_per_day: 1,
        reason: null,
      })
      add(r.repo, sid, ago(start - took), 'session_ended', {})
    }
    for (const s of r.sessions) {
      add(r.repo, s.id, ago(50), 'session_started', {
        cwd: `/wt/${s.id}`,
        source: 'startup',
        worktree: `/wt/${s.id}`,
        branch: 'feat',
      })
      if (s.task)
        add(r.repo, s.id, ago(49), 'task_claimed', {
          task_id: s.task,
          mode: 'confirmed',
          confidence: 1,
        })
      // Earlier finished units with mixed labels (the session drill's timeline).
      const labels = ['on_task', 'on_task', 'detour', 'on_task', 'drift', 'on_task']
      const sources = ['agent', 'heuristic', 'declared', 'owner', 'heuristic', 'agent']
      for (let u = 0; u < (s.units ?? 0); u++) {
        add(r.repo, s.id, ago(49 - u * 6), 'prompt', { chars: 5 })
        add(r.repo, s.id, ago(45 - u * 6), 'unit_stopped', {})
        add(r.repo, s.id, ago(45 - u * 6), 'unit_labeled', {
          unit_no: u + 1,
          label: labels[u % labels.length],
          source: sources[u % sources.length],
        })
      }
      add(r.repo, s.id, ago(s.units ? 8 : 45), 'prompt', { chars: 5 })
      if (s.units)
        add(r.repo, s.id, ago(7), 'usage', {
          path: `/t/${s.id}.jsonl`,
          offset_after: 10,
          replies: [
            {
              message_id: `${s.id}-r`,
              model: 'claude-opus-5-5',
              ts: ago(7),
              input: 20_000,
              output: 30_000,
              cache_read: 400_000,
            },
          ],
        })
      if (s.working) continue
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
