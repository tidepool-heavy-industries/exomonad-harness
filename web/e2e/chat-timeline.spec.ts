import { test, expect } from '@playwright/test'
import type { ChatEntry, ChatTool, ChatPage } from '../src/generated/chat'
import { snapshot, target } from './fixtures.mjs'
import { control, openFixture, observations } from './support'
const hash = 'a'.repeat(64)
const modelOrigin = { kind: 'embedded' as const, ...target }
const origin: ChatPage['origin'] = { run: target.run, nativeActor: '7', incarnation: '1' }
const message = (sequence: number, text: string, id?: string): Extract<ChatEntry, { kind: 'message' }> => ({ sequence: String(sequence), kind: 'message', requestId: 'request-worker-1', position: String(sequence), hash, tool: null, providerItemId: id ?? null, inputOperation: null, item: { type: 'message', role: 'assistant', content: text, ...(id ? { id } : {}) } })
function actorSnapshot() { const data = snapshot(); data.actors = data.actors.map((actor, index) => index === 1 ? { ...actor, outputOrigin: origin, modelHeadRequest: 'request-worker-1' } : actor); return data }
async function openChat(page: any, request: any, entries: unknown[]) {
  await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: entries })
  await page.goto(`/chat/root/worker?run=${target.run}&incarnation=${target.incarnation}`)
  await expect(page.getByRole('heading', { name: '/root/worker', exact: true })).toBeVisible()
}

test('chat latest page loads once and older pages preserve chronology and repeated text', async ({ page, request }) => {
  const entries = Array.from({ length: 130 }, (_, index) => message(index + 1, `Conversation entry ${index + 1}`))
  await openChat(page, request, entries)
  await expect(page.getByText('Conversation entry 130', { exact: true })).toBeVisible()
  await expect(page.getByText('Conversation entry 1', { exact: true })).toHaveCount(0)
  expect((await observations(request)).chatReads[0].before).toBeNull()
  const loadOlder = page.getByRole('button', { name: 'Load older messages', exact: true })
  await loadOlder.scrollIntoViewIfNeeded()
  const priorRow = page.locator('[data-sequence="81"]')
  const priorOffset = await priorRow.evaluate(element => element.getBoundingClientRect().top - element.closest('.worker-chat-scroll')!.getBoundingClientRect().top)
  await loadOlder.click()
  await expect(page.getByText('Conversation entry 31', { exact: true })).toHaveCount(1)
  // Browsers round scrollTop to device pixels; fractional line heights can
  // leave a subpixel remainder without moving the reader's visible position.
  await expect.poll(async () => Math.abs(await priorRow.evaluate(element => element.getBoundingClientRect().top - element.closest('.worker-chat-scroll')!.getBoundingClientRect().top) - priorOffset)).toBeLessThan(1)
  await expect(page.getByText('Conversation entry 130', { exact: true })).toHaveCount(1)
  expect((await observations(request)).chatReads.some((read: any) => read.before === '81')).toBe(true)
  const sequences = await page.getByRole('list', { name: 'Conversation entries' }).locator(':scope > [data-sequence]').evaluateAll(elements => elements.map(element => element.getAttribute('data-sequence')))
  expect(sequences).toEqual(Array.from({ length: 100 }, (_, index) => String(index + 31)))
})

test('chat groups exact issued tools while result markers keep interleaving chronology', async ({ page, request }) => {
  const tool: ChatTool = { operation: { origin: modelOrigin, request: 'issuer', call: 'same' }, invocation: { request: 'issuer', position: '0', hash } }
  const other = { ...tool, operation: { ...tool.operation, request: 'different' }, invocation: { ...tool.invocation, request: 'different' } }
  const entries = [
    { sequence: '1', kind: 'message', requestId: 'issuer', position: '0', hash, tool, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call', call_id: 'same', name: 'Compile', input: 'exact invocation' } },
    message(2, 'Other activity'),
    { sequence: '3', kind: 'message', requestId: 'claimant', position: '0', hash, tool, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call_output', call_id: 'same', output: 'exact result' } },
    { sequence: '4', kind: 'message', requestId: 'claimant-other', position: '0', hash, tool: other, providerItemId: null, inputOperation: null, item: { type: 'custom_tool_call_output', call_id: 'same', output: 'other issuance' } },
  ] satisfies ChatEntry[]
  await openChat(page, request, entries)
  const activity = page.locator('.tool-activity').filter({ has: page.getByRole('heading', { name: /Compile/ }) })
  await expect(activity).toContainText('Result retained')
  await activity.getByText('Execution log', { exact: true }).click()
  await expect(activity).toContainText('exact invocation')
  await expect(activity).toContainText('exact result')
  await expect(activity).not.toContainText('other issuance')
  await activity.getByText('Result details', { exact: true }).click()
  await activity.getByRole('button', { name: 'Show raw result item', exact: true }).click()
  expect(JSON.parse((await activity.getByLabel('Raw result item', { exact: true }).textContent())!)).toMatchObject({ call_id: 'same', output: 'exact result' })
  await expect(page.getByText('The exact invocation is outside the loaded window.', { exact: true })).toBeVisible()
  await expect(page.locator('.tool-result-marker')).toHaveCount(1)
  expect((await observations(request)).commands).toHaveLength(0)
})

test('chat preserves the live element only for an exact issued provider occurrence', async ({ page, request }) => {
  const live = { origin: modelOrigin, requestId: 'request-worker-1', itemId: 'provider-item', channel: 'assistant' as const, index: '0', version: '1', text: 'Live provider preview', overflow: false, streaming: true, committedHash: null }
  const unissued = { ...message(1, 'Submitted item with the same ID'), providerItemId: null, item: { type: 'message', role: 'user', id: 'provider-item', content: 'Submitted item with the same ID' } } satisfies ChatEntry
  const initial = { ...actorSnapshot(), liveOutput: [live, { ...live, origin: { ...modelOrigin, actor: '/different' }, text: 'Unrelated actor preview' }] }
  await openFixture(page, request, { snapshot: initial, chatEntries: [unissued] })
  await page.goto(`/chat/root/worker?run=${target.run}&incarnation=${target.incarnation}`)
  const preview = page.getByText('Live provider preview', { exact: true })
  await expect(preview).toBeVisible()
  await expect(page.getByText('Submitted item with the same ID', { exact: true })).toHaveCount(1)
  await expect(page.getByText('Unrelated actor preview', { exact: true })).toHaveCount(0)
  const row = await preview.locator('xpath=ancestor::*[@data-timeline-key][1]').elementHandle()
  expect(row).not.toBeNull()
  const issued = message(2, 'Published provider final', 'provider-item')
  await control(request, 'config', { chatEntries: [unissued, issued] })
  await expect(page.getByText('Published provider final', { exact: true })).toBeVisible()
  await expect(preview).toHaveCount(0)
  expect(await row!.evaluate(element => element === document.querySelector('[data-sequence="2"]'))).toBe(true)
  await control(request, 'snapshot', { ...initial, seq: '11', liveOutput: [] })
  await expect(page.getByText('Published provider final', { exact: true })).toHaveCount(1)
  expect(await row!.evaluate(element => element === document.querySelector('[data-sequence="2"]'))).toBe(true)
  expect((await observations(request)).commands).toHaveLength(0)
})
