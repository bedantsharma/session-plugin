// Screenshots for the owner's review (not a pass/fail check): the glance, a project and a
// session at 1440×900, saved to test-results/screens/ (git-ignored, never committed).
import { mkdirSync } from 'node:fs'
import { expect, test } from '@playwright/test'
import { serve } from './serve'

const OUT = 'test-results/screens'

test('screenshots: glance, project and session at 1440×900', async ({ page }) => {
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
  } finally {
    s.stop()
  }
})
