// Tracks, hiding, removing and the owner's check against the real binary (spec 2026-11-14).
import { expect, test } from '@playwright/test'
import { serve } from './serve'

const repo = { repo: 'r1', name: 'shop', secondTrack: true, sessions: [{ id: 'idle001' }] }

test('two bars; hide a bar, find it in the drawer, bring it back', async ({ page }) => {
  const s = await serve([repo])
  try {
    await page.goto(s.url)
    const card = page.locator('[data-project="r1"]')
    await expect(card.locator('[data-track]')).toHaveCount(2)
    await expect(card.getByTestId('progress-text')).toHaveCount(0)
    await card.locator('[data-track="2"]').getByRole('button', { name: 'Menu for track 2' }).click()
    await page.getByRole('menuitem', { name: 'Hide this track' }).click()
    await expect(card.locator('[data-track]')).toHaveCount(0)
    await page.getByTestId('hidden-button').click()
    const drawer = page.getByTestId('hidden-drawer')
    await expect(drawer).toContainText('Docs refresh')
    await drawer.getByRole('button', { name: 'Bring back' }).click()
    await page.keyboard.press('Escape')
    await expect(card.locator('[data-track]')).toHaveCount(2)
  } finally {
    s.stop()
  }
})

test('remove a task through the dialog', async ({ page }) => {
  const s = await serve([repo])
  try {
    await page.goto(s.url)
    await page.locator('[data-project="r1"]').getByRole('button', { name: 'shop' }).click()
    const row = page.locator('[data-node="T2-1.2"]')
    await expect(row).toContainText('Add a guide')
    await row.getByRole('button', { name: /Menu for step/ }).click()
    await page.getByRole('menuitem', { name: 'Remove for good' }).click()
    await expect(page.getByTestId('remove-impact')).toContainText("Remove task 1.2 'Add a guide'")
    const confirm = page.getByRole('button', { name: 'Remove for good' }).last()
    await expect(confirm).toBeDisabled()
    await page.getByRole('checkbox').check()
    await confirm.click()
    await expect(page.locator('[data-node="T2-1.2"]')).toHaveCount(0)
  } finally {
    s.stop()
  }
})
