import { test, expect, type Page, type TestInfo } from '@playwright/test'
import { snapshot, target } from './fixtures.mjs'
import { actorSnapshot, entries, form, message, modelOrigin, openChat } from './chat-fixtures'
import { audit, captureViewport, chooseWorker, control, openFixture, view, zoomContext } from './support'

// These captures are review artifacts, not pixel goldens. All content enters
// through the same fixture transport used by the interaction regression suite.
async function capture(page: Page, info: TestInfo, name: string) {
  await captureViewport(page, info, `${name}-viewport`)
  await audit(page, info, name)
}

for (const width of [1280, 390, 320]) {
  test(`visual review: chat and form lifecycle at ${width}px`, async ({ page, request }, info) => {
    await page.setViewportSize({ width, height: width === 1280 ? 800 : 844 })
    await openChat(page, request)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    const firstField = page.getByRole('textbox', { name: 'Project name', exact: true })
    await expect(firstField).toBeFocused()
    await expect.poll(() => firstField.evaluate(element => { const rect = element.getBoundingClientRect(); return document.elementFromPoint(rect.left + rect.width / 2, rect.top + rect.height / 2) === element })).toBe(true)
    await capture(page, info, 'pending-form')
    const cell = page.getByRole('region', { name: 'haskell tool call' })
    await cell.getByRole('heading', { name: 'haskell', exact: true }).scrollIntoViewIfNeeded()
    await capture(page, info, 'tool-source')
    await cell.getByText('Tool evidence', { exact: true }).click()
    await capture(page, info, 'tool-failure')

    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await page.getByRole('textbox', { name: 'Project name', exact: true }).fill('Conversational tooling — 検証')
    const rejected = { ...form, revisionSequence: '6', errors: [{ field: 'project', message: 'Choose a project with an available verification environment.' }] }
    await control(request, 'config', { chatEntries: [...entries.slice(0, -1), { kind: 'form', sequence: form.sequence, form: rejected }] })
    await page.getByRole('button', { name: 'Refresh conversation', exact: true }).click()
    await expect(page.getByText(rejected.errors[0]!.message)).toBeVisible()
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await capture(page, info, 'validation')
    const submitted = { ...form, revisionSequence: '7', state: 'submitted', draft: { project: 'Conversational tooling — 検証', lane: 'focused' }, attemptId: '00000000-0000-4000-8000-000000000011' }
    await control(request, 'config', { chatEntries: [...entries.slice(0, -1), { kind: 'form', sequence: form.sequence, form: submitted }] })
    await page.getByRole('button', { name: 'Refresh conversation', exact: true }).click()
    await expect(page.getByText('Response submitted. Waiting for the actor to process it.')).toBeVisible()
    await page.getByText('Response submitted. Waiting for the actor to process it.').scrollIntoViewIfNeeded()
    await capture(page, info, 'submitted-response')
  })
}

test('visual review: shell views and empty chat', async ({ page, request }, info) => {
  await openFixture(page, request, { snapshot: actorSnapshot() })
  for (const screen of ['Tree', 'Timeline', 'Inbox']) {
    if (screen !== 'Tree') await view(page, screen)
    await capture(page, info, screen.toLowerCase())
  }
  await chooseWorker(page)
  await expect(page.getByText('No conversation entries have been retained yet.')).toBeVisible()
  await capture(page, info, 'empty-chat')
  const standalone = snapshot()
  delete (standalone as any).hostRun
  standalone.actors = []
  await openFixture(page, request, { snapshot: standalone })
  await view(page, 'Command')
  await capture(page, info, 'command')
  await control(request, 'reset', { authenticated: false })
  await page.goto('/')
  await expect(page.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible()
  await capture(page, info, 'authentication')
})

test('visual review: streaming, retained publication, and disconnection', async ({ page, request }, info) => {
  const data = actorSnapshot()
  const text = 'Inspecting the verification environment.\n\nThe current response is streaming; earlier conversation remains readable.'
  const live = { origin: modelOrigin, requestId: 'request-worker-1', itemId: 'visual-live', index: '0', committedHash: null, channel: 'assistant', text, version: '1', streaming: true, overflow: false }
  await openFixture(page, request, { snapshot: { ...data, liveOutput: [live] } })
  await chooseWorker(page)
  await expect(page.getByText('Streaming', { exact: true })).toBeVisible()
  await capture(page, info, 'streaming')
  await control(request, 'config', { chatEntries: [message('1', text, 'visual-live')] })
  await control(request, 'snapshot', { ...data, seq: '11', liveOutput: [] })
  await page.getByRole('button', { name: 'Refresh conversation', exact: true }).click()
  await expect(page.locator('[data-sequence="1"]')).toBeVisible()
  await capture(page, info, 'retained')
  await control(request, 'config', { holdSnapshot: true })
  await control(request, 'disconnect')
  await expect(page.getByText('Host unavailable; retained conversation entries remain visible.')).toBeVisible()
  await capture(page, info, 'disconnected')
})

test('visual review: actual 200 percent zoom', async ({ request }, info) => {
  await control(request, 'reset', { snapshot: actorSnapshot(), chatEntries: entries })
  const { context, page } = await zoomContext(info)
  try {
    await chooseWorker(page)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await capture(page, info, 'zoom-form')
    await page.getByRole('textbox', { name: /^Message / }).scrollIntoViewIfNeeded()
    await capture(page, info, 'zoom-composer')
  } finally { await context.close() }
})


test('visual review: recovery and realistic long content', async ({ page, request }, info) => {
  const data = actorSnapshot()
  const block = { request: 'request-worker-1', generation: '3', reason: { kind: 'continuity_unproven' } }
  const blocked = { ...data, actors: data.actors.map((actor, index) => index === 1 ? { ...actor, inference: { state: 'blocked', block } } : actor) }
  await openFixture(page, request, { snapshot: blocked, chatEntries: [message('1', 'Reviewing `/srv/swarm/checkouts/harness-execution-visibility-aggregate-20261012/web/src/ConversationContent.tsx`.\n\nAleksandra Wiśniewska-Kowalczyk — 検証結果を保持します。')] })
  await chooseWorker(page)
  await page.getByText('Actor controls', { exact: true }).click()
  await expect(page.getByRole('heading', { name: 'Recover inference', exact: true })).toBeVisible()
  await capture(page, info, 'recovery')
  await page.setViewportSize({ width: 320, height: 844 })
  await page.getByRole('radio', { name: 'Retained visible history', exact: true }).scrollIntoViewIfNeeded()
  await capture(page, info, 'recovery-narrow')
})

test('keyboard form submission uses the same HTTP attempt and keeps submitted history readable', async ({ page, request }) => {
  let posts = 0
  await page.route('**/api/actor-form/submit', async route => {
    posts++
    const payload = route.request().postDataJSON()
    expect(payload.draft).toEqual({ project: 'Keyboard verification', lane: 'focused' })
    await route.fulfill({ json: { ...form, revisionSequence: '6', state: 'submitted', attemptId: payload.operationId, draft: payload.draft } })
  })
  await openChat(page, request)
  await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
  await page.getByRole('textbox', { name: 'Project name', exact: true }).fill('Keyboard verification')
  await page.getByRole('textbox', { name: 'Project name', exact: true }).press('Enter')
  await expect(page.getByText('Response submitted. Waiting for the actor to process it.')).toBeVisible()
  await expect(page.getByText('Keyboard verification', { exact: true })).toBeVisible()
  expect(posts).toBe(1)
})

test('short viewport preserves page reading position while the conversation refreshes', async ({ page, request }) => {
  await page.setViewportSize({ width: 800, height: 500 })
  await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: Array.from({ length: 40 }, (_, i) => message(String(i + 1), `Retained message ${i + 1}\n\nA complete thought in the ongoing conversation.`)) })
  await chooseWorker(page)
  const row = page.locator('[data-sequence="20"]')
  await row.scrollIntoViewIfNeeded()
  await expect.poll(() => page.evaluate(() => window.scrollY)).toBeGreaterThan(100)
  const before = await row.evaluate(element => element.getBoundingClientRect().top)
  await control(request, 'config', { chatEntries: Array.from({ length: 41 }, (_, i) => message(String(i + 1), `Retained message ${i + 1}\n\nA complete thought in the ongoing conversation.`)) })
  await expect(page.getByRole('button', { name: 'New activity ↓', exact: true })).toBeAttached()
  const after = await row.evaluate(element => element.getBoundingClientRect().top)
  expect(Math.abs(after - before)).toBeLessThan(1)
  await page.getByRole('button', { name: 'New activity ↓', exact: true }).click()
  await expect(page.locator('[data-sequence="41"]')).toBeAttached()
  await expect.poll(() => page.evaluate(() => document.documentElement.scrollHeight - innerHeight - scrollY)).toBeLessThan(2)
})
