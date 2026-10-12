import { test, expect, type Page, type APIRequestContext } from '@playwright/test'
import type { ChatEntry, ChatTool } from '../src/generated/chat'
import type { StoredActorForm } from '../src/generated/actor-form'
import { snapshot, target } from './fixtures.mjs'
import { audit, chooseWorker, control, openFixture, zoomContext } from './support'

const origin = { run: target.run, nativeActor: '7', incarnation: '1' }
const modelOrigin = { kind: 'embedded' as const, ...target }
const hash = 'a'.repeat(64)
function actorSnapshot() {
  const data = snapshot()
  data.actors = data.actors.map((actor, index) => index === 1 ? { ...actor, outputOrigin: origin, modelHeadRequest: 'request-worker-1' } : actor)
  return data
}
const message = (sequence: string, text: string, id: string | null = null): ChatEntry => ({ sequence, kind: 'message', requestId: 'request-worker-1', position: sequence,
  hash, tool: null, providerItemId: id, inputOperation: null, item: { type: 'message', role: 'assistant', content: text, ...(id ? { id } : {}) } })
const form: StoredActorForm = { sequence: '5', revisionSequence: '5', state: 'open', attemptId: null, draft: null, errors: [], answer: null,
  opening: { origin, mountId: 'choose-validation', conversation: null, execution: { kind: 'actor_program' }, form: { version: 1, root: { kind: 'group', children: [
    { kind: 'section', title: 'Choose the next verification', child: { kind: 'text', id: 'project', label: 'Project name', initial: null } },
    { kind: 'choice', id: 'lane', label: 'Verification lane', initial: 'focused', options: [
      { id: 'focused', label: 'Focused browser checks', presentation: { kind: 'text', text: 'Focused browser checks — streaming, forms, and recovery' } },
      { id: 'full', label: 'Complete browser suite', presentation: { kind: 'text', text: 'Complete browser suite — all existing interactions' } },
    ] },
  ] } } } }
const tool: ChatTool = { operation: { origin: modelOrigin, request: 'issuer', call: 'cell' }, invocation: { request: 'issuer', position: '0', hash } }
const code = ['let checks =', '      [ "streaming selection"', '      , "retained forms"', '      , "reconnect continuity"', '      ]', ...Array.from({ length: 24 }, (_, index) => `-- Verification ${index + 1}: retain exact actor ownership across updates.`), 'traverse_ runCheck checks'].join('\n')
const entries: ChatEntry[] = [
  message('1', '# Verification report\n\n' + Array.from({ length: 12 }, (_, index) => `Step ${index + 1} preserves the conversation while execution continues. You can read earlier results, select source text, and answer a pending form without losing your place.`).join('\n\n') + '\n\nThe complete report ends here.'),
  { sequence: '2', kind: 'message', requestId: 'issuer', position: '0', hash, tool, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call', call_id: 'cell', name: 'haskell', input: code } },
  { sequence: '3', kind: 'message', requestId: 'result', position: '0', hash, tool, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call_output', call_id: 'cell', output: 'Compilation failed\nCell.hs:31:11: Variable not in scope: runCheck\nNo effects were evaluated.' } },
  message('4', 'The compiler could not resolve `runCheck`. Choose a verification lane below while I prepare the corrected cell.'),
  { sequence: '5', kind: 'form', form },
]

async function openChat(page: Page, request: APIRequestContext, chatEntries: ChatEntry[] = entries) {
  await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries })
  await chooseWorker(page)
  await expect(page.getByRole('navigation', { name: 'Pending responses' })).toBeVisible()
}

for (const width of [1280, 390]) {
  test(`code, failed result and pending form remain usable at ${width}px`, async ({ page, request }, testInfo) => {
    await page.setViewportSize({ width, height: 844 })
    await openChat(page, request)
    const report = page.locator('[data-sequence="1"] .message-content-viewport').first()
    expect(await report.evaluate(element => element.clientHeight >= element.scrollHeight - 1)).toBe(true)
    const cell = page.getByRole('region', { name: 'haskell tool call' })
    await cell.getByRole('button', { name: 'Expand Input', exact: true }).click()
    await expect(cell.getByRole('button', { name: 'Collapse Input', exact: true })).toHaveAttribute('aria-expanded', 'true')
    await expect(cell.getByText(/Compilation failed/)).toBeVisible()
    await cell.getByText('Execution log', { exact: true }).click()
    await expect(cell.getByText(/Compiler phases and individual effects are not reported/)).toBeVisible()
    await audit(page, testInfo, `execution-${width}`)
    await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
    await expect(page.getByRole('textbox', { name: 'Project name', exact: true })).toBeFocused()
    await audit(page, testInfo, `pending-form-${width}`)
  })
}

test('form submission updates attention immediately, preserves validation drafts and presents a readable response', async ({ page, request }) => {
  let attempt = 0
  await page.route('**/api/actor-form/submit', async route => {
    const payload = route.request().postDataJSON()
    attempt++
    const result = attempt === 1 ? { ...form, revisionSequence: '6', errors: [{ field: 'project', message: 'Use the full project name' }], draft: payload.draft }
      : { ...form, revisionSequence: '7', state: 'submitted', draft: payload.draft }
    await route.fulfill({ json: result })
  })
  await openChat(page, request)
  await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
  const input = page.getByRole('textbox', { name: 'Project name', exact: true })
  await input.fill('Harness')
  await page.getByRole('button', { name: 'Submit', exact: true }).click()
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
