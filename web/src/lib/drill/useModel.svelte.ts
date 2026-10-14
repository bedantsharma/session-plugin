// Fetch a drill-down model, again whenever the polling cursor moves.
import type { Client } from '$lib/api'

export function useModel<T>(client: () => Client, path: () => string, version: () => number) {
  const s = $state<{ data: T | null; error: string | null }>({ data: null, error: null })
  $effect(() => {
    const p = path()
    version()
    client()
      .get<T>(p)
      .then((d) => {
        s.data = d
        s.error = null
      })
      .catch((e: Error) => {
        s.error = e.message
      })
  })
  return s
}
