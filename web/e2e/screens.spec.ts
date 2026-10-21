// Screenshots for the owner's review (not a pass/fail check): the glance, a project and a
// session at 1440×900, the glance at 1280×720 and the desktop card, saved to
// test-results/screens/ (git-ignored, never committed).
import { mkdirSync } from 'node:fs'
import { resolve } from 'node:path'
import { pathToFileURL } from 'node:url'
import { expect, test } from '@playwright/test'
import { serve } from './serve'

const OUT = 'test-results/screens'

test('screenshots: glance, project, session, a 1280×720 glance and the card', async ({ page }) => {
  mkdirSync(OUT, { recursive: true })
  await page.setViewportSize({ width: 1440, height: 900 })
  // The owner's theme; the light theme follows the system the same way.
  await page.emulateMedia({ colorScheme: 'dark' })
  const s = await serve([
    {
      repo: 'r1',
      name: 'shop',
      milestones: [
        // The seed's own task is "1".
        { id: '2', title: 'Accounts and sign-in', hill: 'downhill' },
        { id: '3', title: 'Catalogue and search', hill: 'uphill' },
        { id: '4', title: 'Cart and checkout', hill: 'uphill' },
        { id: '5', title: 'Order history' },
      ],
      sessions: [
        { id: 'w0rk1ng', working: true, units: 6, task: '2.1' },
        { id: 'red0001', red: true },
        { id: 'idle002' },
      ],
    },
    { repo: 'r2', name: 'api', sessions: [{ id: 'b1', red: true }] },
  ])
  try {
    await page.goto(s.url)
    await expect(page.getByTestId('pill')).toBeVisible()
    await page.screenshot({ path: `${OUT}/glance.png`, fullPage: true })

    await page.locator('[data-project="r1"]').getByRole('button', { name: 'shop' }).click()
    await expect(page.getByRole('heading', { level: 2, name: /shop/ })).toBeVisible()
    await page.screenshot({ path: `${OUT}/project.png`, fullPage: true })

    await page.getByRole('button', { name: '← Glance' }).click()
    await page
      .locator('[data-session="w0rk1ng"]')
      .getByRole('button', { name: /S w0rk1n/ })
      .click()
    await expect(page.getByText('Time by label')).toBeVisible()
    await page.screenshot({ path: `${OUT}/session.png`, fullPage: true })

    await page.getByRole('button', { name: '← Glance' }).click()
    await page.setViewportSize({ width: 1280, height: 720 })
    await expect(page.getByTestId('pill')).toBeVisible()
    await page.screenshot({ path: `${OUT}/glance-1280x720.png` })

    // The card on its own page, with a stand-in host: answer ui/initialize, then send the
    // glance as show_progress's tool result.
    const token = /token=([^&]+)/.exec(s.url)![1]
    const glance = await (
      await page.request.get(new URL('/api/glance', s.url).toString(), {
        headers: { Authorization: `Bearer ${token}` },
      })
    ).json()
    await page.addInitScript((g) => {
      window.addEventListener('message', (e) => {
        const m = e.data as { id?: number; method?: string }
        if (m.method === 'ui/initialize')
          window.postMessage(
            { jsonrpc: '2.0', id: m.id, result: { hostContext: { theme: 'dark' } } },
            '*',
          )
        if (m.method === 'ui/notifications/initialized')
          window.postMessage(
            {
              jsonrpc: '2.0',
              method: 'ui/notifications/tool-result',
              params: { structuredContent: g },
            },
            '*',
          )
      })
    }, glance)
    await page.setViewportSize({ width: 640, height: 900 })
    await page.goto(pathToFileURL(resolve('dist-card/card.html')).toString())
    await expect(page.getByTestId('pill')).toBeVisible()
    await page.getByRole('button', { name: 'show milestones' }).first().click()
    await page.screenshot({ path: `${OUT}/card.png`, fullPage: true })
  } finally {
    s.stop()
  }
})
