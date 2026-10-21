// Owner-facing words for states the model keeps as codes (plan §2). Display only: the
// model and API keep `uphill`/`downhill`, `on_task`/`detour`/`drift` and the light codes.
import type { SessionRow } from './types'
import jargon from './jargon.json'

export type ChipTone = 'ok' | 'warn' | 'bad' | 'muted' | 'clay'

export interface Word {
  icon: string
  text: string
  tone: ChipTone
  tip?: string
}

/** A milestone's hill, or null when there is none or it is finished. */
export function hillWord(hill: string | null, donePct: number): Word | null {
  if (donePct >= 100) return null
  if (hill === 'uphill')
    return {
      icon: '?',
      text: 'exploring',
      tone: 'warn',
      tip: 'Unknowns remain — this % may still change',
    }
  if (hill === 'downhill')
    return { icon: '→', text: 'executing', tone: 'ok', tip: 'Path is clear — this % is reliable' }
  return null
}

/** A finished or troubled parent (milestone or goal) as a chip; null otherwise. */
export function rollupWord(status: string | null | undefined): Word | null {
  if (status === 'done') return { icon: '✓', text: 'done', tone: 'ok' }
  if (status === 'needs_attention') return { icon: '✗', text: 'needs attention', tone: 'bad' }
  return null
}

/** A milestone's hill chip: hidden once the milestone is done, by % or by its tasks. */
export function milestoneHill(m: { hill: string | null; done_pct: number; status?: string }) {
  return hillWord(m.hill, m.status === 'done' ? 100 : m.done_pct)
}

/** A session's status chip: the state in words with a distinct shape per state. */
export function sessionWord(s: Pick<SessionRow, 'state_label' | 'light' | 'light_word'>): Word {
  const tone: ChipTone = s.light === 'green' ? 'ok' : s.light === 'amber' ? 'warn' : 'bad'
  switch (s.state_label) {
    case 'working': {
      const extra = s.light_word === 'on task' ? '' : ` · ${s.light_word}`
      return { icon: '●', text: `Working${extra}`, tone }
    }
    case 'done':
      return { icon: '✓', text: 'Done', tone: 'ok' }
    case 'needs you':
      return { icon: '■', text: 'Needs you', tone: 'bad' }
    case 'ended':
      return { icon: '○', text: 'Ended', tone: 'muted' }
    default:
      return { icon: '◆', text: 'Idle', tone: 'warn' }
  }
}

const SOURCE: Record<string, string> = {
  agent: 'agent',
  declared: 'agent',
  heuristic: 'auto',
  auto: 'auto',
  owner: 'owner',
}

/** A unit label in words: "✓ On task · agent", "★ Asked for · agent", "Unlabelled". */
export function unitWord(label: string | null, source: string | null): Word {
  const suffix = source ? ` · ${SOURCE[source] ?? source}` : ''
  switch (label) {
    case 'on_task':
      return { icon: '✓', text: `On task${suffix}`, tone: 'ok' }
    case 'detour':
      return { icon: '↷', text: `Detour${suffix}`, tone: 'warn' }
    case 'drift':
      return { icon: '▲', text: `Drift${suffix}`, tone: 'bad' }
    case 'requested':
      return { icon: '★', text: `Asked for${suffix}`, tone: 'ok' }
    default:
      return { icon: '', text: 'Unlabelled', tone: 'muted' }
  }
}

/** `6 min`, `<1 min`, `185 min`. */
export function mins(secs: number): string {
  return secs < 60 ? '<1 min' : `${Math.round(secs / 60)} min`
}

/** `no files`, `1 file`, `4 files`. */
export function filesWord(n: number): string {
  return n === 0 ? 'no files' : n === 1 ? '1 file' : `${n} files`
}

const READBACK: Record<string, string> = {
  not_needed: 'not needed',
  pending: 'not confirmed yet',
  ok: 'confirmed',
  mismatch: 'misunderstood — check what it said',
}

/** Whether the new session confirmed it understood the handoff. */
export function readbackWord(state: string): string {
  return READBACK[state] ?? state.replaceAll('_', ' ')
}

const LEFTOVER: Record<string, string> = {
  open: 'open',
  report_only: 'for information only',
  resolved: 'resolved',
}

const LEFTOVER_ACTION: Record<string, string> = {
  adopt: 'adopted',
  resume: 'resumed',
  undo: 'undone',
}

const LEFTOVER_SUGGESTION: Record<string, string> = {
  adopt: 'adopt (commit it)',
  resume: 'resume (carry on with it)',
  undo: 'undo (put it aside, recoverable)',
}

/** A leftover's state: "adopted", or "open — suggested: resume (carry on with it)". */
export function leftoverWord(l: {
  state: string
  suggested: string
  resolution: string | null
}): string {
  if (l.resolution) return LEFTOVER_ACTION[l.resolution] ?? l.resolution
  const suggested = LEFTOVER_SUGGESTION[l.suggested] ?? l.suggested
  return `${LEFTOVER[l.state] ?? l.state.replaceAll('_', ' ')} — suggested: ${suggested}`
}

const ANSWER: Record<string, string> = {
  continue: 'continue',
  make_task: 'make it a task',
  stop: 'stop',
}

/** The owner's drift answer in words. */
export function answerWord(answer: string): string {
  return ANSWER[answer] ?? answer.replaceAll('_', ' ')
}

/** A plan node's status: its icon, its words and its colour class. */
export const STATUS: Record<string, { icon: string; word: string; cls: string }> = {
  todo: { icon: '○', word: 'to do', cls: 'text-muted' },
  active: { icon: '◔', word: 'being worked on', cls: 'text-clay' },
  claimed: { icon: '◑', word: 'said done, not checked yet', cls: 'text-clay' },
  l1: { icon: '◕', word: 'code written, tests not run', cls: 'text-clay' },
  l2: { icon: '✓', word: 'tests pass', cls: 'text-ok' },
  l3: { icon: '✓', word: 'tried end to end', cls: 'text-ok' },
  l4: { icon: '✓', word: 'you signed it off', cls: 'text-ok' },
  failed: { icon: '✗', word: 'failed a check', cls: 'text-bad' },
  in_progress: { icon: '◔', word: 'in progress', cls: 'text-clay' },
  done: { icon: '✓', word: 'done', cls: 'text-ok' },
  needs_attention: { icon: '✗', word: 'needs attention', cls: 'text-bad' },
  dropped: { icon: '–', word: 'dropped', cls: 'text-muted' },
}

export function statusWord(status: string): { icon: string; word: string; cls: string } {
  return STATUS[status] ?? { icon: '·', word: status.replaceAll('_', ' '), cls: 'text-muted' }
}

/** A level of the verification ladder in words (the L-codes stay in the model). */
export function levelWord(level: number): string {
  return statusWord(level <= 0 ? 'claimed' : `l${Math.min(level, 4)}`).word
}

const BY: Record<string, string> = {
  auto: 'bridge-screen',
  agent: 'the agent',
  rerun: 'bridge-screen re-ran the tests',
  owner: 'you',
  owner_via_agent: 'you, through the agent',
}

const RERUN: Record<string, string> = {
  passed: 'they passed',
  failed: 'they failed',
  timed_out: 'they took too long',
  not_configured: 'no test command set',
}

const VERIF_STATE: Record<string, string> = {
  current: '',
  stale: 'out of date: files changed since',
  failed: 'undone by a later failed check',
  withdrawn: 'taken back',
}

/** One verification row: "tests pass · by the agent · out of date: files changed since". */
export function verificationWord(v: {
  level: number
  by: string
  rerun: string | null
  state: string
}): string {
  const by = BY[v.by] ?? v.by.replaceAll('_', ' ')
  const rerun = v.rerun ? ` (${RERUN[v.rerun] ?? v.rerun.replaceAll('_', ' ')})` : ''
  const state = VERIF_STATE[v.state] ?? v.state.replaceAll('_', ' ')
  return [`${levelWord(v.level)}`, `by ${by}${rerun}`, state].filter(Boolean).join(' · ')
}

/** A session flag in words (the same words as `status` and `watch`). */
export const FLAG: Record<string, string> = {
  checkpoint_missed: "didn't save its work when asked",
  handoff_unstable: 'started from an unclear handoff',
  squash_failed: "couldn't tidy its commits",
  checkpoint_pending: 'work not saved yet',
  readback_pending: "hasn't confirmed it understood the handoff yet",
  verify_pending: 'said done, not checked yet',
}

const JARGON = new Set(jargon.words.map((w) => w.toLowerCase()))

/** The raw codes in `text` an owner should never see (empty when it reads in words). */
export function jargonIn(text: string): string[] {
  const out: string[] = []
  for (const token of text.split(/[^A-Za-z0-9_-]+/)) {
    if (!token) continue
    const t = token.toLowerCase()
    if (
      JARGON.has(t) ||
      /^l[0-4]$/.test(t) ||
      /^[a-z0-9]+(_[a-z0-9]+)+$/.test(t) ||
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/.test(t)
    )
      out.push(token)
  }
  return out
}
