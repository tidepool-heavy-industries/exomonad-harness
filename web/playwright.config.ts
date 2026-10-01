import { defineConfig, chromium } from '@playwright/test'
import { existsSync } from 'node:fs'
import { resolve } from 'node:path'

if (!process.env.PLAYWRIGHT_BROWSERS_PATH) throw new Error('Enter the pinned nix develop .#web shell; PLAYWRIGHT_BROWSERS_PATH is required.')
const executablePath = chromium.executablePath()
if (!existsSync(executablePath)) throw new Error(`Materialize nix build .#web-chromium; browser missing: ${executablePath}`)

const evidenceDir = process.env.HARNESS_BROWSER_EVIDENCE_DIR ?? resolve('../target/gui-browser')

export default defineConfig({
  testDir: './e2e',
  testMatch: '**/*.spec.ts',
  workers: 1,
  fullyParallel: false,
  retries: 0,
  timeout: 30_000,
  outputDir: resolve(evidenceDir, 'artifacts'),
  reporter: [['list'], ['json', { outputFile: resolve(evidenceDir, 'results.json') }]],
  use: {
    baseURL: 'http://127.0.0.1:4387',
    viewport: { width: 1280, height: 800 },
    contextOptions: { reducedMotion: 'reduce' },
    launchOptions: { executablePath },
    trace: 'retain-on-failure',
    screenshot: 'only-on-failure',
  },
  webServer: {
    command: 'node e2e/server.mjs',
    url: 'http://127.0.0.1:4387',
    reuseExistingServer: false,
    timeout: 10_000,
  },
})
