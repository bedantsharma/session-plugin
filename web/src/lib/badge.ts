// The tab title and favicon carry the needs-you count (spec §10.4): `(3) bridge-screen`,
// and a red square with the number — a different shape, not only a different colour.

export const BASE_TITLE = 'bridge-screen'

export function titleFor(count: number, connected: boolean): string {
  if (!connected) return `(?) ${BASE_TITLE}`
  return count > 0 ? `(${count}) ${BASE_TITLE}` : BASE_TITLE
}

/** The favicon: the clay circle, or a red square with the count while N > 0. */
export function faviconFor(count: number): string {
  if (count <= 0) return '/favicon.svg'
  const n = count > 99 ? '99' : String(count)
  const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 32 32"><rect x="1" y="1" width="30" height="30" rx="4" fill="#c0392b"/><text x="16" y="23" font-size="${n.length > 1 ? 16 : 20}" font-weight="700" text-anchor="middle" fill="#ffffff" font-family="sans-serif">${n}</text></svg>`
  return `data:image/svg+xml,${encodeURIComponent(svg)}`
}

export function applyBadge(count: number, connected: boolean, doc: Document = document) {
  doc.title = titleFor(count, connected)
  let link = doc.querySelector<HTMLLinkElement>('link[rel="icon"]')
  if (!link) {
    link = doc.createElement('link')
    link.rel = 'icon'
    doc.head.appendChild(link)
  }
  link.href = faviconFor(connected ? count : 0)
}
