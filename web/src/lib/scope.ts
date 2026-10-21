// The project's "Scope added" line (spec §5.6): every step up of the scope line, with
// several made in the same minute read as one entry ("3 additions, +45 min at 10:02")
// rather than one entry each.
import { hhmm } from './format'

export interface ScopeStep {
  ts: string
  total_min: number
}

export interface AddedGroup {
  /** The time of the first addition in the minute. */
  ts: string
  /** How many additions were made in that minute. */
  count: number
  /** Minutes of scope they added together. */
  by: number
}

const minute = (ts: string) => Math.floor(new Date(ts).getTime() / 60_000)

/** The steps up of the scope line, grouped by the minute they were made in. */
export function addedGroups(scope: ScopeStep[]): AddedGroup[] {
  const out: AddedGroup[] = []
  scope.forEach((p, i) => {
    if (i === 0 || p.total_min <= scope[i - 1].total_min) return
    const by = p.total_min - scope[i - 1].total_min
    const last = out.at(-1)
    if (last && minute(last.ts) === minute(p.ts)) {
      last.count += 1
      last.by += by
    } else {
      out.push({ ts: p.ts, count: 1, by })
    }
  })
  return out
}

/** `+20 min at 10:02 · 3 additions, +45 min at 10:05`. */
export function addedText(groups: AddedGroup[]): string {
  return groups
    .map((g) =>
      g.count === 1
        ? `+${g.by} min at ${hhmm(g.ts)}`
        : `${g.count} additions, +${g.by} min at ${hhmm(g.ts)}`,
    )
    .join(' · ')
}
