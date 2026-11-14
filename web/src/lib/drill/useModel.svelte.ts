// Fetch a drill-down model, again whenever the polling cursor moves.
import { ApiError, type Client } from '$lib/api'

export function useModel<T>(client: () => Client, path: () => string, version: () => number) {
  const s = $state<{ data: T | null; error: string | null; gone: boolean }>({
    data: null,
    error: null,
    gone: false,
  })
  $effect(() => {
    const p = path()
    version()
    client()
      .get<T>(p)
      .then((d) => {
        s.data = d
        s.error = null
        s.gone = false
      })
      .catch((e: Error) => {
        // A page that no longer exists says so in words.
        s.gone = e instanceof ApiError && e.status === 404
        s.error = s.gone ? 'That page is gone.' : e.message
      })
  })
  return s
}
