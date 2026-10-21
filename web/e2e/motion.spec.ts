// Motion gating in a real browser (plan §5, WCAG 2.2.2): the working session's ring and
// the Live dot loop by default, stop when the header's Motion switch is off (and stay off
// after a reload), and never run under prefers-reduced-motion.
import { type Page, expect, test } from '@playwright/test'
import { serve } from './serve'

const loops = (page: Page) =>
  page.evaluate(() => ({
    ring: getComputedStyle(document.querySelector('.working-dot')!, '::after').animationName,
    live: getComputedStyle(document.querySelector('.live-dot')!).animationName,
  }))

test('loops run with Motion on and stop with it off or under reduced motion', async ({ page }) => {
  const s = await serve([
    { repo: 'r1', name: 'shop', sessions: [{ id: 'w0rk1ng', working: true }] },
  ])
  try {
    await page.goto(s.url)
    await expect(page.locator('[data-working] .working-dot')).toBeVisible()
    await expect(page.getByTestId('live')).toBeVisible()
    expect(await loops(page)).toEqual({ ring: 'ring', live: 'breathe' })

    const toggle = page.getByRole('button', { name: 'Motion' })
    await expect(toggle).toHaveAttribute('aria-pressed', 'true')
    await toggle.click()
    await expect(toggle).toHaveAttribute('aria-pressed', 'false')
    expect(await page.evaluate(() => document.documentElement.dataset.motion)).toBe('off')
    expect(await loops(page)).toEqual({ ring: 'none', live: 'none' })

    // Remembered across a reload.
    await page.reload()
    await expect(page.locator('[data-working] .working-dot')).toBeVisible()
    expect(await loops(page)).toEqual({ ring: 'none', live: 'none' })
    await page.getByRole('button', { name: 'Motion' }).click()
    expect(await loops(page)).toEqual({ ring: 'ring', live: 'breathe' })

    // Reduced motion wins even with the switch on.
    await page.emulateMedia({ reducedMotion: 'reduce' })
    expect(await loops(page)).toEqual({ ring: 'none', live: 'none' })
  } finally {
    s.stop()
  }
})
