// The glance end to end: the real binary on a seeded temp home, a needs-you item, a
// Continue click, the item leaving the queue; the header and queue above the fold at
// 1280×720.
import { expect, test } from '@playwright/test'
import { serve } from './serve'

test('a needs-you item leaves the queue after Continue', async ({ page }) => {
  const s = await serve([{ repo: 'r1', name: 'shop', sessions: [{ id: 'red0001', red: true }] }])
  try {
    await page.goto(s.url)
    const queue = page.getByRole('region', { name: 'Needs you' })
    await expect(queue.getByText('red000 · far off its task (40 min)')).toBeVisible()
    await expect(page.getByTestId('pill')).toHaveText(/1 need you/)
    await expect(page).toHaveTitle('(1) bridge-screen')
    // The token left the address bar.
    expect(page.url()).not.toContain('token')
    await queue.getByRole('button', { name: 'Continue' }).click()
    await expect(queue.getByText('Told red0001: continue.')).toBeVisible()
    await page.mouse.move(0, 700)
    await expect(page.getByTestId('all-clear')).toBeVisible({ timeout: 10_000 })
    await expect(page.getByRole('region', { name: 'Needs you' })).toHaveCount(0)
  } finally {
    s.stop()
  }
})

test('the header and the needs-you queue sit above the fold at 1280×720', async ({ page }) => {
  // The glance may scroll now (projects and sessions are below), but the header and the
  // 4 most urgent items must be fully visible without it.
  const s = await serve([
    {
      repo: 'r1',
      name: 'shop',
      sessions: [
        { id: 'a1', red: true },
        { id: 'a2', red: true },
      ],
    },
    { repo: 'r2', name: 'api', sessions: [{ id: 'b1', red: true }, { id: 'b2' }] },
    { repo: 'r3', name: 'docs', sessions: [{ id: 'c1', red: true }, { id: 'c2' }] },
  ])
  try {
    await page.goto(s.url)
    await expect(page.getByTestId('pill')).toHaveText(/4 need you/)
    const queue = page.getByRole('region', { name: 'Needs you' })
    await expect(queue.locator('[data-item]')).toHaveCount(4)
    const fold = await page.evaluate(() => {
      const bottom = (el: Element | null) => (el ? el.getBoundingClientRect().bottom : Infinity)
      const header = document.querySelector('header')
      const queue = document.querySelector('[aria-label="Needs you"]')
      return {
        header: bottom(header) <= window.innerHeight,
        queue: bottom(queue) <= window.innerHeight,
        scrollX: document.documentElement.scrollWidth <= window.innerWidth,
        vw: window.innerWidth,
        vh: window.innerHeight,
      }
    })
    expect(fold).toEqual({ header: true, queue: true, scrollX: true, vw: 1280, vh: 720 })
    // A phone: one column, no sideways scroll.
    await page.setViewportSize({ width: 375, height: 812 })
    expect(
      await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth),
    ).toBe(true)
  } finally {
    s.stop()
  }
})

test('no sideways overflow at phone, 1024 and 1100 widths; the columns follow the spec', async ({
  page,
}) => {
  const s = await serve([
    {
      repo: 'r1',
      name: 'shop',
      milestones: [
        { id: '2', title: 'Accounts and sign-in with a long enough title', hill: 'downhill' },
        { id: '3', title: 'Catalogue and search', hill: 'uphill' },
      ],
      sessions: [
        { id: 'w0rk1ng', working: true, units: 6, task: '2.1' },
        { id: 'red0001', red: true },
      ],
    },
  ])
  // Elements poking past the right edge, whether or not the page itself scrolls.
  const overflow = () =>
    page.evaluate(() =>
      [...document.querySelectorAll('body *')]
        .filter((el) => !el.closest('.sr-only'))
        .filter((el) => el.getBoundingClientRect().right > window.innerWidth + 0.5)
        .map((el) => `${el.tagName.toLowerCase()}.${el.className}`.slice(0, 120)),
    )
  const order = () =>
    page.evaluate(() => {
      const top = (label: string) =>
        document.querySelector(`[aria-label="${label}"]`)!.getBoundingClientRect().top
      return top('Live sessions') < top('Projects') ? 'sessions first' : 'projects first'
    })
  try {
    for (const [width, first] of [
      [375, 'sessions first'],
      [1024, 'projects first'],
      [1100, 'projects first'],
    ] as const) {
      await page.setViewportSize({ width, height: 800 })
      if (width === 375) await page.goto(s.url)
      await expect(page.locator('[data-milestone="3"]')).toBeVisible()
      expect(await overflow(), `glance at ${width}`).toEqual([])
      expect(await order(), `glance at ${width}`).toBe(first)

      await page.locator('[data-project="r1"]').getByRole('button', { name: 'shop' }).click()
      await expect(page.getByRole('heading', { name: 'Plan' })).toBeVisible()
      expect(await overflow(), `project at ${width}`).toEqual([])

      await page.getByRole('button', { name: '← Glance' }).click()
      await page
        .locator('[data-session="w0rk1ng"]')
        .getByRole('button', { name: /S w0rk1n/ })
        .click()
      await expect(page.getByText('Time by label')).toBeVisible()
      expect(await overflow(), `session at ${width}`).toEqual([])
      await page.getByRole('button', { name: '← Glance' }).click()
    }
  } finally {
    s.stop()
  }
})
