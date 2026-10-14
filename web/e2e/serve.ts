// Starts the real binary on a seeded temp home and returns the dashboard URL.
import { type ChildProcess, spawn } from 'node:child_process'
import { existsSync, mkdtempSync, rmSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join, resolve } from 'node:path'
import { seed, type Seed } from './seed'

const bin =
  process.env.BRIDGE_BIN ??
  [resolve('../target/release/bridge-screen'), resolve('../target/debug/bridge-screen')].find((p) =>
    existsSync(p),
  ) ??
  'bridge-screen'

export async function serve(repos: Seed[]): Promise<{ url: string; stop: () => void }> {
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
