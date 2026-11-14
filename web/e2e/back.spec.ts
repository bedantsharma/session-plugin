// Back and the swipe stay inside the dashboard (spec 2026-11-14 §5).
import { expect, test } from '@playwright/test'
import { serve } from './serve'

const repo = { repo: 'r1', name: 'shop', sessions: [{ id: 'idle001' }] }

test('open project, open task, Back, Back lands on the glance; reload keeps the screen', async ({
  page,
}) => {
  const s = await serve([repo])
  try {
    await page.goto(s.url)
    await expect(page.getByTestId('all-clear')).toBeVisible()
    await page.locator('[data-project="r1"]').getByRole('button', { name: 'shop' }).click()
    await expect(page).toHaveURL(/#\/project\/r1$/)
    await page.getByRole('button', { name: 'Login form' }).first().click()
    await expect(page).toHaveURL(/#\/task\/r1\/1$/)
    await page.goBack()
    await expect(page).toHaveURL(/#\/project\/r1$/)
    await expect(page.getByRole('heading', { level: 2, name: /shop/ })).toBeVisible()
    // A reload on the project page stays on it (the token is kept for the session).
    await page.reload()
    await expect(page.getByRole('heading', { level: 2, name: /shop/ })).toBeVisible()
    await page.goBack()
    await expect(page.locator('[data-project="r1"]')).toBeVisible()
  } finally {
    s.stop()
  }
})

test('a page that is gone says so and the glance replaces it', async ({ page }) => {
  const s = await serve([repo])
  try {
    await page.goto(s.url)
    await expect(page.getByTestId('all-clear')).toBeVisible()
    await page.evaluate(() => (window.location.hash = '#/project/nope'))
    await expect(page.getByTestId('page-gone')).toContainText('That page is gone')
  } finally {
    s.stop()
  }
})
