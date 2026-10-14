import { defineConfig, devices } from '@playwright/test'

// One smoke test against the real binary (spec §14). Build it first:
// `cargo build --release` (or set BRIDGE_BIN), after `npm run build`.
export default defineConfig({
  testDir: 'e2e',
  timeout: 60_000,
  use: { ...devices['Desktop Chrome'], viewport: { width: 1280, height: 720 } },
  projects: [{ name: 'chromium', use: { browserName: 'chromium' } }],
})
