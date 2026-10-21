// The read models the binary serves (crates/bridge-screen-core/src/view). Screens render
// these as they are: ordering, words and lights are decided server-side.

export type Light = 'green' | 'amber' | 'red'
export type NeedsKind = 'drift_red' | 'question' | 'permission_wait' | 'verify_overdue' | 'leftover'
export type Tone = 'primary' | 'neutral' | 'danger'

export interface NodeRef {
  id: string
  title: string
}

/** An owner action, sent back exactly as the server offered it. */
export type OwnerAction = { kind: string } & Record<string, unknown>

export interface ActionInput {
  name: string
  label: string
  required: boolean
}

export interface ActionSpec {
  label: string
  action: OwnerAction
  inputs: ActionInput[]
  tone: Tone
}

export interface RespondIn {
  session_short: string
  worktree: string | null
}

export interface NeedsYou {
  id: string
  kind: NeedsKind
  repo_id: string
  repo_name: string
  session_id: string | null
  task: NodeRef | null
  title: string
  detail: string
  files?: string[]
  actions: ActionSpec[]
  respond_in: RespondIn | null
  rank: number
  age_s: number
}

export interface SessionFlag {
  kind: string
  age_s: number | null
  tasks?: string[]
}

export type Rest =
  { kind: 'needs_you'; why: NeedsKind } | { kind: 'done'; task: NodeRef } | { kind: 'idle' }

/** Tokens and money, computed by the server from the configured prices. */
export interface CostRow {
  tokens: number
  tokens_text: string
  /** USD of the priced models; null when nothing is priced (unknown, never $0). */
  usd: number | null
  unpriced_models: string[]
  /** `$12.40`, `≥ $12.40 (some models unpriced)` or `cost unknown`. */
  text: string
  /** `$12.40 spent · 3.1M tokens`. */
  summary: string
}

export interface Forecast {
  p50_hours: number
  p90_hours: number
  p50_date: string | null
  p90_date: string | null
  remaining_min: number
  unestimated: number
  points: number
  hours_per_day: number | null
  reason: string | null
  median_ratio?: number | null
}

/** A project's forecast with its words (dates only when `compact` is set). */
export interface ForecastRow extends Forecast {
  /** `likely by Oct 30 · could take until Nov 8`, or the reason there are no dates. */
  text: string
  compact: string | null
  short: string
  /** What the two dates mean, for the tooltip and screen readers. */
  help: string
  basis: string
}

export interface SessionRow {
  session_id: string
  short: string
  repo_id: string
  state_label: string
  light: Light
  light_word: string
  task: NodeRef | null
  detour: { label: string; reason: string } | null
  units: number
  active_min: number
  nav_text: string
  flags: SessionFlag[]
  rest: Rest | null
  asked_question: boolean
  branch: string | null
  worktree: string | null
  cost: CostRow | null
  rank: number
}

export interface MilestoneRow {
  id: string
  title: string
  hill: 'uphill' | 'downhill' | null
  done_pct: number
  claimed_pct: number
  stuck: boolean
  stuck_sessions: number
}

export interface ProjectRow {
  repo_id: string
  name: string
  goal: NodeRef | null
  done_pct: number
  claimed_pct: number
  unestimated: number
  milestones: MilestoneRow[]
  leftovers: { open: number; report_only: number }
  tuned: string | null
  forecast: ForecastRow | null
  cost: CostRow | null
  needs_you: number
  sessions: SessionRow[]
}

export interface Glance {
  as_of: string
  last_event_pos: number
  live_sessions: number
  projects: ProjectRow[]
  needs_you: NeedsYou[]
  needs_you_count: number
  escalated: boolean
}

export interface TreeNode {
  id: string
  parent_id: string | null
  kind: string
  title: string
  intent: string | null
  status: string
  hill: string | null
  stale: boolean
  owner_checked: boolean
  estimate_min: number | null
  actual_min: number
  depends_on: string[]
  decisions: number
  depth: number
}

export interface ProjectDrill {
  repo_id: string
  name: string
  tree: TreeNode[]
  scope: { ts: string; total_min: number; done_min: number }[]
  replans: { ts: string; reason: string | null; op_count: number; session_id: string }[]
}

export interface UnitRow {
  unit_no: number
  started_at: string
  stopped_at: string | null
  active_s: number
  task_id: string | null
  label: string | null
  label_source: string | null
  drift_score: number | null
  detour: string | null
  files: number
  disagreement: boolean
  tokens: number
  tokens_text: string | null
}

export interface SessionDrill {
  session_id: string
  short: string
  repo_id: string
  state: string
  branch: string | null
  worktree: string | null
  task: NodeRef | null
  units: UnitRow[]
  split: {
    on_task_s: number
    requested_s: number
    drift_s: number
    unlabeled_s: number
    detours: { label: string; reason: string; secs: number }[]
  }
  suspended: { item_id: string; what: string; resume_cue: string; since: string }[]
  checkpoints: { sha: string; task_id: string | null; next_action: string | null; ts: string }[]
  leftovers: {
    leftover_id: string
    path: string
    task_id: string | null
    suggested: string
    state: string
    resolution: string | null
  }[]
  handoff_note: { in_flight: string; if_then: string[]; ts: string } | null
  readback: string
  owner_answers: { ts: string; answer: string; reason: string | null }[]
  cost: CostRow | null
}

export interface Decision {
  decision_id: string
  what: string
  why: string
  alternatives: string[]
  supersedes: string | null
  superseded_by: string | null
  ts: string
}

export interface TaskDrill {
  repo_id: string
  id: string
  title: string
  status: string | null
  estimate_min: number | null
  actual_min: number
  decisions: Decision[][]
  verifications: {
    verif_id: string
    level: number
    by: string
    rerun: string | null
    sha: string
    evidence: string
    state: string
    ts: string
  }[]
  actors: { agent_type: string | null; model: string | null }[]
  retries: number
  usage: TaskUsage | null
  steps: { id: string; title: string; status: string; current: boolean }[]
  /** Actual ÷ estimate, once done. */
  ratio: number | null
}

export interface CostSplit {
  label: string
  tokens: number
  tokens_text: string
  cost_text: string
}

export interface TaskUsage {
  total: CostRow
  by_model: CostSplit[]
  by_agent: CostSplit[]
}

export interface CalibrationPoint {
  task_id: string
  title: string
  estimate_min: number
  actual_min: number
  ratio: number
  model: string
  completed_at: string
}

export interface Price {
  input: number
  output: number
  cache_read: number
  cache_write_5m: number
  cache_write_1h: number
}

export interface CalibrationDrill {
  repo_id: string
  name: string
  min_points: number
  model_points: number
  models: {
    model: string
    name: string
    points: CalibrationPoint[]
    median_ratio: number
    text: string
    own: boolean
  }[]
  pooled: CalibrationPoint[]
  pooled_text: string | null
  forecast: ForecastRow
  history: (Forecast & { ts: string })[]
  prices: { model: string; name: string; price: Price; source: string }[]
  unpriced_models: string[]
  notes: string[]
}

export interface HistoryPage {
  events: {
    pos: number
    ts: string
    kind: string
    session_id: string
    task_id: string | null
    data: unknown
  }[]
  next_before: number | null
}

export interface TuningDrill {
  repo_id: string
  current: { drift_lower: number; drift_upper: number; tolerance_pct: number }
  defaults: { drift_lower: number; drift_upper: number; tolerance_pct: number }
  tuned: boolean
  samples: number
  changes: { ts: string; key: string; old: number | null; new: number; samples: number }[]
  revert: ActionSpec | null
}
