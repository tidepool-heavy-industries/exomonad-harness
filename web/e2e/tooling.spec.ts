import { test, expect, chromium } from '@playwright/test'
import { resolve } from 'node:path'

// These smoke cases validate tooling against the production entry point. The
// behavioral acceptance suite separately verifies the joined GUI candidate.
test('production assets use the isolated authenticated wire fixture', async ({ page, request }) => {
  await request.post('/__fixture/reset', { data: {} })
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Tree', exact: true })).toBeVisible()
  await expect(page.getByRole('region', { name: 'Worker hierarchy' })).toContainText('/root/worker')
  await expect.poll(async () => (await (await request.get('/__fixture/observations')).json()).connections).toBeGreaterThan(0)
})

test('pinned Chromium applies actual 200 percent browser zoom', async ({ request }, testInfo) => {
  await request.post('/__fixture/reset', { data: {authenticated:false} })
  const extension = resolve('e2e/zoom-extension')
  const context = await chromium.launchPersistentContext(testInfo.outputPath('zoom-profile'), {
    headless: true,
    viewport: null,
    executablePath: chromium.executablePath(),
    args: ['--window-size=1280,800', `--disable-extensions-except=${extension}`, `--load-extension=${extension}`],
  })
  try {
    const worker = context.serviceWorkers()[0] ?? await context.waitForEvent('serviceworker')
    const page = await context.newPage()
    await page.goto('http://127.0.0.1:4387/')
    await expect(page.getByRole('heading', {name:'Operator sign in'})).toBeVisible()
    const before = await page.evaluate(() => ({width:innerWidth,ratio:devicePixelRatio}))
    const zoom = await worker.evaluate(async () => {
      // Test-only extension API; this changes desktop browser zoom, not pinch
      // scaling, CSS zoom, viewport emulation, or device scale emulation.
      const chrome = (globalThis as any).chrome
      const tabs = await chrome.tabs.query({})
      const tab = tabs.find((item: any) => item.url?.startsWith('http://127.0.0.1:4387/'))
      await chrome.tabs.setZoom(tab.id, 2)
      return chrome.tabs.getZoom(tab.id)
    })
    expect(zoom).toBe(2)
    await expect.poll(() => page.evaluate(() => devicePixelRatio)).toBe(before.ratio * 2)
    const after = await page.evaluate(() => ({width:innerWidth,ratio:devicePixelRatio}))
    expect(after.width).toBe(before.width / 2)
    await expect(page.getByRole('heading', {name:'Operator sign in'})).toBeVisible()
    await testInfo.attach('actual-browser-zoom', {body:JSON.stringify({zoom,before,after,browser:context.browser()?.version()}), contentType:'application/json'})
    await page.screenshot({path:testInfo.outputPath('actual-200-percent-sign-in.png'),fullPage:true})
  } finally { await context.close() }
})
