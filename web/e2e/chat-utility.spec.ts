import { test, expect, type Page } from '@playwright/test'
import { actorSnapshot, entries } from './chat-fixtures'
import { target } from './fixtures.mjs'
import { audit, captureViewport, chooseWorker, control, observations, openFixture } from './support'

async function switchWorker(page: Page, name: string) {
  await page.getByRole('button', { name: /^Worker:/ }).click()
  await page.getByRole('complementary', { name: 'Workers' }).getByRole('link', { name, exact: true }).click()
}

for (const width of [320, 390]) {
  test(`narrow chat preserves drafts and makes the active form visible at ${width}px`, async ({ page, request }, info) => {
    await page.setViewportSize({ width, height: 844 })
    await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: entries })
    await chooseWorker(page)
    const input = page.getByRole('textbox', { name: 'Message /root/worker', exact: true })
    const compact = await input.boundingBox()
    expect(compact!.height).toBeLessThan(50)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    const banner = await page.getByRole('navigation', { name: 'Pending responses' }).boundingBox()
    const heading = await page.getByRole('heading', { name: 'Form · response required', exact: true }).boundingBox()
    const viewport = await page.getByRole('region', { name: 'Worker conversation', exact: true }).boundingBox()
    expect(heading!.y).toBeGreaterThanOrEqual(banner!.y + banner!.height)
    const usableHeight = viewport!.y + viewport!.height - Math.max(viewport!.y, banner!.y + banner!.height)
    expect(usableHeight).toBeGreaterThan(330)
    await info.attach('task-space', { body: JSON.stringify({ width, usableHeight, banner, viewport, compact }), contentType: 'application/json' })
    await captureViewport(page, info, `utility-${width}-viewport`)
    await input.fill('Keep my worker draft\nwhile I answer the form')
    await expect.poll(async () => (await input.boundingBox())!.height).toBeGreaterThanOrEqual(76)
    await switchWorker(page, '/root')
    await expect(page.getByRole('textbox', { name: 'Message /root', exact: true })).toHaveValue('')
    await page.getByRole('textbox', { name: 'Message /root', exact: true }).fill('A separate root draft')
    await switchWorker(page, '/root/worker')
    await expect(input).toHaveValue('Keep my worker draft\nwhile I answer the form')
    await page.getByRole('button', { name: /^Worker:/ }).click()
    await page.getByRole('searchbox', { name: 'Find worker' }).press('Escape')
    await expect(page.getByRole('button', { name: /^Worker:/ })).toBeFocused()
    await expect(page.getByRole('searchbox', { name: 'Find worker' })).not.toBeVisible()
    expect((await observations(request)).commands).toHaveLength(0)
  })
}

test('composer Retry targets the current block without a separate input request', async ({ page, request }, info) => {
  const data = actorSnapshot()
  const block = { request: 'request-worker-1', generation: '3', reason: { kind: 'continuity_unproven' } }
  const blocked = (generation: string, seq: string) => ({ ...data, seq, actors: data.actors.map((actor, i) => i === 1 ? { ...actor, inference: { state: 'blocked', block: { ...block, generation } } } : actor) })
  await openFixture(page, request, { snapshot: blocked('3', '10'), chatEntries: entries })
  await chooseWorker(page)
  const retry = page.getByRole('button', { name: 'Retry', exact: true })
  await expect(retry).toBeVisible()
  expect((await observations(request)).commands).toHaveLength(0)
  await captureViewport(page, info, 'recovery-entry-viewport')
  await control(request, 'snapshot', blocked('4', '11'))
  await retry.click()
  await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
  expect((await observations(request)).commands[0].command).toEqual({ action: 'recover', target, expected_block: { ...block, generation: '4' }, mode: 'exact' })
})

test('unfocused form and composer boundaries meet contrast and diagnostics remain readable', async ({ page, request }, info) => {
  await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: entries })
  await chooseWorker(page)
  const result = await page.evaluate(() => {
    const luminance = (color: string) => {
      const values = color.match(/[\d.]+/g)!.slice(0, 3).map(Number).map(value => { const c = value / 255; return c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4 })
      return values[0]! * .2126 + values[1]! * .7152 + values[2]! * .0722
    }
    const contrast = (a: string, b: string) => { const x = luminance(a), y = luminance(b); return (Math.max(x, y) + .05) / (Math.min(x, y) + .05) }
    return {
      inputs: Array.from(document.querySelectorAll<HTMLInputElement | HTMLTextAreaElement>('.mounted-form input[type="text"], .worker-chat-composer textarea')).map(element => {
        const style = getComputedStyle(element)
        const surface = getComputedStyle(element.closest('.mounted-form, .worker-chat-composer')!)
        return contrast(style.borderTopColor, surface.backgroundColor)
      }),
      diagnostics: Array.from(document.querySelectorAll('.timeline-metadata, .chat-attention small')).map(element => parseFloat(getComputedStyle(element).fontSize)),
    }
  })
  expect(result.inputs).toHaveLength(2)
  for (const ratio of result.inputs) expect(ratio).toBeGreaterThanOrEqual(3)
  expect(result.diagnostics.length).toBeGreaterThan(0)
  for (const size of result.diagnostics) expect(size).toBeGreaterThanOrEqual(12)
  await info.attach('contrast-and-type', { body: JSON.stringify(result), contentType: 'application/json' })
  await audit(page, info, 'utility-contrast')
})

test.describe('touch controls', () => {
  test.use({ hasTouch: true, viewport: { width: 390, height: 844 } })
  test('chat controls have forgiving hit areas on coarse pointers', async ({ page, request }) => {
    await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: entries })
    await chooseWorker(page)
    expect(await page.evaluate(() => matchMedia('(pointer: coarse)').matches)).toBe(true)
    for (const name of ['Jump to form', 'Send input', 'Interrupt']) {
      const rect = await page.getByRole('button', { name, exact: true }).boundingBox()
      expect(rect!.height).toBeGreaterThanOrEqual(44)
    }
  })
})
