import { test, expect, type Page, type APIRequestContext } from '@playwright/test'
import type { ChatEntry, ChatTool } from '../src/generated/chat'
import type { StoredActorForm } from '../src/generated/actor-form'
import { snapshot, target } from './fixtures.mjs'
import { audit, chooseWorker, control, openFixture, zoomContext } from './support'

import { origin, modelOrigin, actorSnapshot, message, form, tool, code, entries, openChat } from './chat-fixtures'

for (const width of [1280, 390]) {
  test(`code, failed result and pending form remain usable at ${width}px`, async ({ page, request }, testInfo) => {
    await page.setViewportSize({ width, height: 844 })
    await openChat(page, request)
    const report = page.locator('[data-sequence="1"] .message-content-viewport').first()
    expect(await report.evaluate(element => element.clientHeight >= element.scrollHeight - 1)).toBe(true)
    const cell = page.getByRole('region', { name: 'haskell tool call' })
    await cell.getByRole('button', { name: 'Expand Input', exact: true }).click()
    await expect(cell.getByRole('button', { name: 'Collapse Input', exact: true })).toHaveAttribute('aria-expanded', 'true')
    await expect(cell.locator('.tool-results').getByText(/Compilation failed/)).toBeVisible()
    await cell.getByText('Tool evidence', { exact: true }).click()
    await expect(cell.getByText(/Compiler phases and individual effects are not reported/)).toBeVisible()
    await audit(page, testInfo, `execution-${width}`)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await expect(page.getByRole('textbox', { name: 'Project name', exact: true })).toBeFocused()
    await audit(page, testInfo, `pending-form-${width}`)
  })
}

test('form submission updates attention immediately, preserves validation drafts and presents a readable response', async ({ page, request }) => {
  let attempt = 0
  let submitted!: StoredActorForm
  await page.route('**/api/actor-form/submit', async route => {
    const payload = route.request().postDataJSON()
    attempt++
    submitted = { ...form, revisionSequence: attempt === 1 ? '6' : '8', state: 'submitted', attemptId: payload.operationId, draft: payload.draft }
    await route.fulfill({ json: submitted })
  })
  await openChat(page, request)
  await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
  const input = page.getByRole('textbox', { name: 'Project name', exact: true })
  await input.fill('Harness')
  await page.getByRole('button', { name: 'Submit', exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Form · response submitted' })).toBeVisible()
  await control(request, 'config', { chatEntries: entries.map(entry => entry.kind === 'form' ? { ...entry, form: { ...submitted, revisionSequence: '7', state: 'open', errors: [{ field: 'project', message: 'Use the full project name' }] } } : entry) })
  await expect(input).toBeFocused()
  await expect(input).toHaveAttribute('aria-invalid', 'true')
  await expect(input).toHaveValue('Harness')
  await input.fill('Exomonad Harness')
  await page.getByRole('button', { name: 'Submit', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Pending responses' })).toHaveCount(0)
  await expect(page.getByText('Exomonad Harness', { exact: true })).toBeVisible()
  await expect(page.getByText('Focused browser checks', { exact: true })).toBeVisible()
  await expect(page.getByRole('textbox', { name: 'Project name', exact: true })).toHaveCount(0)
  await page.getByRole('button', { name: 'Refresh conversation', exact: true }).click()
  await expect(page.getByRole('navigation', { name: 'Pending responses' })).toHaveCount(0)
})

for (const stage of ['submit response', 'native validation']) {
  test(`form response respects focus moved elsewhere before ${stage}`, async ({ page, request }) => {
    let release!: () => void
    let submitted!: StoredActorForm
    const waiting = new Promise<void>(resolve => { release = resolve })
    await page.route('**/api/actor-form/submit', async route => {
      const payload = route.request().postDataJSON()
      submitted = { ...form, revisionSequence: '6', state: 'submitted', attemptId: payload.operationId, draft: payload.draft }
      await waiting
      await route.fulfill({ json: submitted })
    })
    await openChat(page, request)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await page.getByRole('textbox', { name: 'Project name', exact: true }).fill('Harness')
    await page.getByRole('button', { name: 'Submit', exact: true }).click()
    const refresh = page.getByRole('button', { name: 'Refresh conversation', exact: true })
    if (stage === 'submit response') await refresh.focus()
    release()
    await expect(page.getByRole('heading', { name: 'Form · response submitted' })).toBeVisible()
    if (stage === 'native validation') await refresh.focus()
    await expect(refresh).toBeFocused()
    await control(request, 'config', { chatEntries: entries.map(entry => entry.kind === 'form' ? { ...entry, form: { ...submitted, revisionSequence: '7', state: 'open', errors: [{ field: 'project', message: 'Use the full project name' }] } } : entry) })
    await expect(page.getByRole('textbox', { name: 'Project name', exact: true })).toHaveAttribute('aria-invalid', 'true')
    await expect(refresh).toBeFocused()
  })
}

test('streaming and retained publication preserve selected text and the reading position', async ({ page, request }) => {
  const text = 'A stable passage to select while the agent continues.'
  const live = { origin: modelOrigin, requestId: 'request-worker-1', itemId: 'streamed', channel: 'assistant' as const, index: '0', version: '1', text, streaming: true, overflow: false, committedHash: null }
  const initial = { ...actorSnapshot(), liveOutput: [live] }
  await openFixture(page, request, { snapshot: initial, chatEntries: [] })
  await chooseWorker(page)
  const body = page.locator('.message-content-body').filter({ hasText: text }).first()
  await expect(body).toBeVisible()
  await body.evaluate(element => {
    const node = element.querySelector('p')!.firstChild!
    window.getSelection()!.setBaseAndExtent(node, 2, node, 16)
  })
  await page.evaluate(() => new Promise<void>(resolve => requestAnimationFrame(() => resolve())))
  const selected = await page.evaluate(() => window.getSelection()!.toString())
  const before = await page.locator('.worker-chat-scroll').evaluate(element => element.scrollTop)
  const full = text + '\n\n' + 'Another paragraph of streamed content.\n\n'.repeat(25)
  await control(request, 'snapshot', { ...initial, seq: '11', liveOutput: [{ ...live, text: full, version: '2' }] })
  await expect(body).toContainText('Another paragraph')
  expect(await page.evaluate(() => window.getSelection()!.toString())).toBe(selected)
  expect(await page.locator('.worker-chat-scroll').evaluate(element => element.scrollTop)).toBe(before)
  await control(request, 'config', { chatEntries: [message('1', full, 'streamed')] })
  await expect(page.locator('[data-sequence="1"]')).toBeVisible()
  expect(await page.evaluate(() => window.getSelection()!.toString())).toBe(selected)
  expect(await page.locator('.worker-chat-scroll').evaluate(element => element.scrollTop)).toBe(before)
})

test('pending form and code controls work at actual 200 percent browser zoom', async ({ request }, testInfo) => {
  await control(request, 'reset', { snapshot: actorSnapshot(), chatEntries: entries })
  const { context, page } = await zoomContext(testInfo)
  try {
    await chooseWorker(page)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await expect(page.getByRole('textbox', { name: 'Project name', exact: true })).toBeFocused()
    await audit(page, testInfo, 'chat-usability-zoom')
  } finally { await context.close() }
})
