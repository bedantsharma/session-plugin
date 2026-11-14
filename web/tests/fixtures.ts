import type {
  ForecastRow,
  Glance,
  HiddenItem,
  NeedsYou,
  ProjectRow,
  TrackRow,
} from '../src/lib/types'

export function item(over: Partial<NeedsYou> = {}): NeedsYou {
  return {
    id: 'drift_red:s1aaaa',
    kind: 'drift_red',
    repo_id: 'r1',
    repo_name: 'shop',
    session_id: 's1aaaa',
    task: { id: '1.1', shown: '1.1', title: 'Login form' },
    title: 's1aaaa · far off its task (40 min)',
    detail: 'detour 40m · D1 "CI cache" · on 1.1 Login form',
    actions: [
      {
        label: 'Continue',
        action: { kind: 'drift', session: 's1aaaa', answer: 'continue' },
        inputs: [{ name: 'reason', label: 'reason (optional)', required: false }],
        tone: 'primary',
      },
      {
        label: 'Make it a task',
        action: { kind: 'drift', session: 's1aaaa', answer: 'make_task', parent: '1' },
        inputs: [
          { name: 'parent', label: 'Under (e.g. 10)', required: true },
          { name: 'title', label: 'Task title', required: true },
        ],
        tone: 'neutral',
      },
      {
        label: 'Stop',
        action: { kind: 'drift', session: 's1aaaa', answer: 'stop' },
        inputs: [],
        tone: 'danger',
      },
    ],
    respond_in: null,
    rank: 0,
    age_s: 600,
    subject: 'card:drift_red:s1aaaa',
    stamp: 11,
    ...over,
  }
}

export function leftover(): NeedsYou {
  const act = (a: string) => ({
    kind: 'resolve',
    repo: 'r1',
    leftover_ids: ['lab'],
    action: a,
  })
  return item({
    id: 'leftover:r1:dead',
    kind: 'leftover',
    title: 'S dead00 left 1 uncommitted file',
    detail: 'suggested: undo · in /wt/dead',
    files: ['src/a.rs'],
    actions: ['Adopt', 'Resume', 'Undo'].map((l) => ({
      label: l,
      action: act(l.toLowerCase()),
      inputs: [],
      tone: l === 'Undo' ? 'primary' : 'neutral',
    })),
  })
}

export function question(id = 'q1'): NeedsYou {
  return item({
    id: `question:${id}`,
    kind: 'question',
    session_id: id,
    title: `${id} asked you a question`,
    detail: '5 min ago · answer in that session',
    actions: [],
    respond_in: { session_short: id, worktree: '/wt/feat-x' },
    rank: 1,
  })
}

export function project(over: Partial<ProjectRow> = {}): ProjectRow {
  return {
    repo_id: 'r1',
    name: 'shop',
    goal: { id: 'G', shown: 'G', title: 'Online shop' },
    goal_status: 'in_progress',
    done_pct: 62,
    claimed_pct: 14,
    run: { number: 1, closed: 0, finished: null },
    unestimated: 0,
    milestones: [
      {
        id: '1',
        shown: '1',
        title: 'Accounts',
        status: 'in_progress',
        hill: 'uphill',
        done_pct: 40,
        claimed_pct: 0,
        stuck: true,
        stuck_sessions: 3,
      },
      {
        id: '2',
        shown: '2',
        title: 'Cart',
        status: 'in_progress',
        hill: 'downhill',
        done_pct: 80,
        claimed_pct: 0,
        stuck: false,
        stuck_sessions: 0,
      },
    ],
    leftovers: { open: 1, report_only: 0 },
    tuned: null,
    forecast: null,
    cost: null,
    needs_you: 1,
    sessions: [],
    subject: 'project:r1',
    stamp: 21,
    tracks: [],
    hidden_tracks: 0,
    reviewed_tasks: 0,
    ...over,
  }
}

export function glance(items: NeedsYou[] = [item()]): Glance {
  return {
    as_of: '2026-10-14T09:00:00Z',
    last_event_pos: 7,
    live_sessions: 1,
    projects: [project()],
    needs_you: items,
    needs_you_count: items.length,
    escalated: items.length > 4,
    branch_groups: [],
    hidden: [],
  }
}

/** A forecast with dates (5 verified tasks), or with only a reason when `reason` is set. */
export function forecast(reason: string | null = null): ForecastRow {
  return {
    p50_hours: 10,
    p90_hours: 20,
    p50_date: reason ? null : '2026-10-30',
    p90_date: reason ? null : '2026-11-08',
    remaining_min: 600,
    unestimated: 0,
    points: reason ? 1 : 5,
    hours_per_day: 2,
    reason,
    text: reason ?? 'likely by Oct 30 · could take until Nov 8',
    compact: reason ? null : 'Oct 30–Nov 8',
    short: reason ? 'forecast needs 2 more finished tasks' : 'likely Oct 30–Nov 8',
    help: `50% and 90% chance, based on how long ${reason ? 1 : 5} verified tasks actually took`,
    basis: 'based on 5 finished tasks; tasks typically take 1.5× their estimate',
  }
}

export const COST = {
  tokens: 3_100_000,
  tokens_text: '3.1M',
  usd: 12.4,
  unpriced_models: [],
  text: '$12.40',
  summary: '$12.40 spent · 3.1M tokens',
}

/** One open track with its own goal and bar. */
export function track(no: number, over: Partial<TrackRow> = {}): TrackRow {
  return {
    track: no,
    goal: {
      id: no === 1 ? 'G' : `T${no}-G`,
      shown: 'G',
      title: no === 1 ? 'Online shop' : 'Docs refresh',
    },
    goal_status: 'in_progress',
    done_pct: no === 1 ? 40 : 10,
    claimed_pct: 0,
    reviewed_tasks: 0,
    run: { number: 1, closed: 0, finished: null },
    unestimated: 0,
    milestones: [],
    forecast: null,
    subject: `track:r1:${no}`,
    stamp: 30 + no,
    finish: {
      label: 'Finish this track',
      action: { kind: 'close_track', repo: 'r1', track: no },
      inputs: [],
      tone: 'neutral',
    },
    remove: {
      text: `Remove the track 'Docs refresh' for good? 7 tasks, 2 done. 1 session is working on it. It leaves the bars, the queue and the forecast and can't be brought back from the dashboard. The history keeps a record.`,
      action: {
        kind: 'remove',
        repo: 'r1',
        id: no === 1 ? 'G' : `T${no}-G`,
        title: 'Docs refresh',
        confirm: false,
      },
    },
    ...over,
  }
}

export function hiddenItem(over: Partial<HiddenItem> = {}): HiddenItem {
  return {
    subject: 'card:verify_overdue:r1:1.2',
    kind: 'card',
    label: 'Check 1.2 Password reset',
    repo_id: 'r1',
    repo_name: 'shop',
    hidden_at: '2026-10-14T08:00:00Z',
    age_s: 3600,
    stamp: 40,
    unhide: { kind: 'unhide', repo: 'r1', subject: 'card:verify_overdue:r1:1.2' },
    ...over,
  }
}
