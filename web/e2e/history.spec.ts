// The History panel end to end: plain sentences by default, raw details only on request.
import { expect, test } from '@playwright/test'
import { serve } from './serve'

test('history reads as sentences and keeps the raw data behind "details"', async ({ page }) => {
  const s = await serve([
    { repo: 'r1', name: 'shop', sessions: [{ id: 'w0rk1ng', working: true, units: 2, task: '1' }] },
  ])
  try {
    await page.goto(s.url)
    await page.locator('[data-project="r1"]').getByRole('button', { name: 'shop' }).click()
    await page.getByRole('button', { name: 'History' }).click()
    await expect(page.getByRole('heading', { level: 2, name: 'History' })).toBeVisible()
    const rows = page.locator('[data-event]')
    await expect(rows.first()).toBeVisible()
    // No raw payloads by default, and the kind filter is a menu.
    await expect(page.getByTestId('raw')).toHaveCount(0)
    await expect(page.getByRole('combobox', { name: 'kind of event' })).toBeVisible()
    await rows
      .first()
      .getByRole('button', { name: /details/ })
      .click()
    await expect(page.getByTestId('raw')).toHaveCount(1)
  } finally {
    s.stop()
  }
})
