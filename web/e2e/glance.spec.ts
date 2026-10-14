// The glance end to end: the real binary on a seeded temp home, a needs-you item, a
// Continue click, the item leaving the queue; and the one-screen layout at 1280×720.
import { type ChildProcess, spawn } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { expect, test } from '@playwright/test'
import { seed, type Seed } from './seed'

const bin =
  process.env.BRIDGE_BIN ??
  [resolve('../target/release/bridge-screen'), resolve('../target/debug/bridge-screen')].find((p) =>
    existsSync(p),
  ) ??
  'bridge-screen'

async function serve(repos: Seed[]): Promise<{ url: string; stop: () => void }> {
  const home = mkdtempSync(join(tmpdir(), 'bridge-screen-e2e-'))
  seed(bin, home, repos)
  const child: ChildProcess = spawn(bin, ['ui', '--no-open', '--port', '0'], {
    env: { ...process.env, BRIDGE_SCREEN_HOME: home },
  })
  const url = await new Promise<string>((ok, fail) => {
    let out = ''
    child.stdout?.on('data', (d: Buffer) => {
      out += d.toString()
      const m = /bridge-screen dashboard: (\S+)/.exec(out)
      if (m) ok(m[1])
    })
    child.on('exit', (code) => fail(new Error(`bridge-screen ui exited ${code}`)))
  })
  return {
    url,
    stop: () => {
      child.kill()
      rmSync(home, { recursive: true, force: true })
    },
  }
}

test('a needs-you item leaves the queue after Continue', async ({ page }) => {
  const s = await serve([{ repo: 'r1', name: 'shop', sessions: [{ id: 'red0001', red: true }] }])
  try {
    await page.goto(s.url)
    const queue = page.getByRole('region', { name: 'Needs you' })
    await expect(queue.getByText('S red000 is far off its task')).toBeVisible()
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

test('the glance fits 1280×720 without scrolling', async ({ page }) => {
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
    // The page can't scroll (main is viewport-high with overflow hidden), so check that
    // nothing inside main is clipped either.
    const fit = await page.evaluate(() => {
      const main = document.querySelector('main')!
      return {
        page: document.documentElement.scrollHeight <= window.innerHeight,
        main: main.scrollHeight <= main.clientHeight,
        vw: window.innerWidth,
        vh: window.innerHeight,
      }
    })
    expect(fit).toEqual({ page: true, main: true, vw: 1280, vh: 720 })
  } finally {
    s.stop()
  }
})
