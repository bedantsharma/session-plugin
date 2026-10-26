// History rows in plain words. The server builds each sentence (so the command line and
// the dashboard agree); this only covers a row without one (an older server) by naming its
// kind in words, and shapes the filter menu.
import type { HistoryEvent, HistoryPage } from '$lib/types'

/** `brand_new_thing` → `Brand new thing`. */
export function kindWords(kind: string): string {
  const spaced = kind.replaceAll('_', ' ').trim()
  return spaced ? spaced[0].toUpperCase() + spaced.slice(1) : 'Something happened'
}

/** The sentence for a row: the server's, else the kind's name in words. */
export function sentence(e: Pick<HistoryEvent, 'summary' | 'kind'>): string {
  return e.summary?.trim() || kindWords(e.kind)
}

/** The task a row is about: `1.2 Login form`, or just the number; null for none. */
export function taskText(
  e: Pick<HistoryEvent, 'task_id' | 'task_shown' | 'task_title'>,
): string | null {
  const shown = e.task_shown ?? e.task_id
  if (!shown) return null
  return e.task_title ? `${shown} ${e.task_title}` : shown
}

/** The filter menu: the kinds present, by plain label. */
export function kindOptions(kinds: HistoryPage['kinds']): { kind: string; label: string }[] {
  return [...(kinds ?? [])]
    .map((k) => ({ kind: k.kind, label: k.label || kindWords(k.kind) }))
    .sort((a, b) => a.label.localeCompare(b.label))
}
