// Small display helpers.

/** `HH:MM` in local time. */
export function hhmm(t: number | string | null): string {
  if (t === null) return '--:--'
  const d = new Date(t)
  return `${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`
}

/** `45s`, `12m`, `3h`, `2d`. */
export function age(secs: number | null): string {
  if (secs === null) return ''
  if (secs < 60) return `${secs}s`
  if (secs < 3600) return `${Math.floor(secs / 60)}m`
  if (secs < 86400) return `${Math.floor(secs / 3600)}h`
  return `${Math.floor(secs / 86400)}d`
}

export function minutes(secs: number): string {
  return `${Math.round(secs / 60)}m`
}

/** How a session flag reads, with its age. */
export function flagText(f: { kind: string; age_s: number | null; tasks?: string[] }): string {
  const words: Record<string, string> = {
    checkpoint_missed: 'checkpoint missed',
    handoff_unstable: 'handoff unstable',
    squash_failed: 'squash failed',
    checkpoint_pending: 'checkpoint pending',
    readback_pending: 'read-back pending',
    verify_pending: 'verify pending',
  }
  const tasks = f.tasks?.length ? `: ${f.tasks.join(', ')}` : ''
  const a = f.age_s === null ? '' : ` · ${age(f.age_s)}`
  return `${words[f.kind] ?? f.kind.replaceAll('_', ' ')}${tasks}${a}`
}
