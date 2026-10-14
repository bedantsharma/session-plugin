// Owner-facing words for states the model keeps as codes (plan §2). Display only: the
// model and API keep `uphill`/`downhill`, `on_task`/`detour`/`drift` and the light codes.
import type { SessionRow } from './types'

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

/** A unit label in words: "✓ On task · agent", "↷ Detour · auto", "Unlabelled". */
export function unitWord(label: string | null, source: string | null): Word {
  const suffix = source ? ` · ${SOURCE[source] ?? source}` : ''
  switch (label) {
    case 'on_task':
      return { icon: '✓', text: `On task${suffix}`, tone: 'ok' }
    case 'detour':
      return { icon: '↷', text: `Detour${suffix}`, tone: 'warn' }
    case 'drift':
      return { icon: '▲', text: `Drift${suffix}`, tone: 'bad' }
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
