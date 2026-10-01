import { expect, chromium, type Page, type APIRequestContext, type TestInfo } from '@playwright/test'
import AxeBuilder from '@axe-core/playwright'
import { resolve } from 'node:path'
import { target } from './fixtures.mjs'

export const baseURL = 'http://127.0.0.1:4387'
export async function control(request: APIRequestContext, action: string, data: unknown = {}) {
  const response = await request.post(`/__fixture/${action}`, { data })
  expect(response.ok()).toBe(true)
  return response.json()
}
export async function observations(request: APIRequestContext) {
  return (await request.get('/__fixture/observations')).json()
}
export async function openFixture(page: Page, request: APIRequestContext, config = {}) {
  await control(request, 'reset', config)
  await page.goto('/')
  await expect(page.getByRole('heading', { name: 'Tree', exact: true })).toBeVisible()
}
export async function view(page: Page, name: string) {
  await page.getByRole('navigation', { name: 'Views' }).getByRole('button', { name, exact: true }).click()
  await expect(page.getByRole('heading', { name, exact: true })).toBeVisible()
}
export async function chooseWorker(page: Page) {
  await view(page, 'Host')
  await page.getByLabel('Target actor', { exact: true }).selectOption(JSON.stringify([target.run,target.actor,target.incarnation]))
  await expect(page.getByLabel('Message to selected actor', {exact:true})).toBeEnabled()
}
export async function noOverflow(page: Page) {
  const dimensions = await page.evaluate(() => ({width:innerWidth,documentWidth:document.documentElement.scrollWidth}))
  expect(dimensions.documentWidth).toBeLessThanOrEqual(dimensions.width + 1)
}
export async function audit(page: Page, testInfo: TestInfo, name: string) {
  const results = await new AxeBuilder({page}).analyze()
  await testInfo.attach(`${name}-axe`, {body:JSON.stringify(results),contentType:'application/json'})
  await page.screenshot({path:testInfo.outputPath(`${name}.png`),fullPage:true})
  expect(results.violations, JSON.stringify(results.violations.map(v=>({id:v.id,impact:v.impact,nodes:v.nodes.map(n=>({target:n.target,summary:n.failureSummary}))})),null,2)).toEqual([])
  await noOverflow(page)
}
export async function zoomContext(testInfo: TestInfo) {
  const extension = resolve('e2e/zoom-extension')
  const context = await chromium.launchPersistentContext(testInfo.outputPath('zoom-profile'), {
    headless:true, viewport:null, executablePath:chromium.executablePath(),
    args:['--window-size=1280,800',`--disable-extensions-except=${extension}`,`--load-extension=${extension}`],
  })
  const worker = context.serviceWorkers()[0] ?? await context.waitForEvent('serviceworker')
  const page = await context.newPage()
  await page.goto(baseURL)
  const before = await page.evaluate(()=>({width:innerWidth,ratio:devicePixelRatio}))
  const zoom = await worker.evaluate(async()=>{
    const chrome=(globalThis as any).chrome
    const tabs=await chrome.tabs.query({})
    const tab=tabs.find((t:any)=>t.url?.startsWith('http://127.0.0.1:4387/'))
    await chrome.tabs.setZoom(tab.id,2)
    return chrome.tabs.getZoom(tab.id)
  })
  expect(zoom).toBe(2)
  await expect.poll(()=>page.evaluate(()=>devicePixelRatio)).toBe(before.ratio*2)
  const after=await page.evaluate(()=>({width:innerWidth,ratio:devicePixelRatio}))
  expect(after.width).toBe(before.width/2)
  await testInfo.attach('actual-browser-zoom',{body:JSON.stringify({zoom,before,after,browser:context.browser()?.version()}),contentType:'application/json'})
  return {context,page}
}
