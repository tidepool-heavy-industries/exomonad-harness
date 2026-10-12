import { test, expect } from '@playwright/test'
import { snapshot, target } from './fixtures.mjs'
import type { ChatEntry } from '../src/generated/chat'
import { audit, chooseWorker, control, observations, openFixture } from './support'

for (const width of [1280, 390]) {
  test(`chat composer remains anchored with one history scroll owner at ${width}px`, async ({ page, request }, testInfo) => {
    await page.setViewportSize({ width, height: 844 })
    await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: Array.from({ length: 50 }, (_, n) => message(String(n + 1), null, `History ${n + 1}`)) })
    await chooseWorker(page)
    const input = page.getByLabel('Message to selected actor', { exact: true })
    await expect(page.getByRole('list', { name: 'Conversation entries' })).toBeVisible()
    const initial = await input.boundingBox()
    expect(initial).not.toBeNull()
    expect(initial!.y + initial!.height).toBeLessThanOrEqual(844)
    const scrolling = await page.locator('.worker-chat-scroll').evaluate(element => {
      const scroll = element as HTMLElement
      const overflowing = scroll.scrollHeight > scroll.clientHeight
      scroll.scrollTop = scroll.scrollHeight
      return overflowing
    })
    expect(scrolling).toBe(true)
    const after = await input.boundingBox()
    expect(after!.y).toBe(initial!.y)
    await expect(page.getByRole('button', { name: 'Retire', exact: true })).not.toBeVisible()
    await expect(page.getByRole('region', { name: 'Retained browser operations' })).not.toBeVisible()
    await audit(page, testInfo, `chat-workspace-${width}`)
  })
}

test('multiline composer keyboard submission is IME-safe and preserves the exact operation payload', async ({ page, request }) => {
  await openFixture(page, request, { receipt: 'none', snapshot: actorSnapshot() })
  await chooseWorker(page)
  const input = page.getByLabel('Message to selected actor', { exact: true })
  await input.fill('First line')
  await input.press('End')
  await input.press('Enter')
  await page.keyboard.type('  Unicode 🐚 and spaces  ')
  const exact = await input.inputValue()
  expect(exact).toContain('\n')
  expect((await observations(request)).commands).toHaveLength(0)
  await input.dispatchEvent('compositionstart')
  await input.press('Control+Enter')
  expect((await observations(request)).commands).toHaveLength(0)
  await expect(input).toHaveValue(exact)
  await input.dispatchEvent('compositionend')
  await input.press('Control+Enter')
  await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
  const first = (await observations(request)).commands[0]
  expect(first.command.text).toBe(exact)
  await expect(input).toHaveValue('')
  await expect(page.locator('[data-input-operation-id]')).toHaveAttribute('data-input-operation-id', first.operation_id)
  await expect(page.locator('[data-input-operation-id]')).toHaveAttribute('data-input-state', 'queued')
  await expect(page.locator('[data-input-operation-id]')).toHaveAttribute('data-input-authority', 'status')
  await page.locator('summary').filter({ hasText: /^Host operations$/ }).click()
  await expect(page.locator(`[data-operation-id="${first.operation_id}"]`)).toBeVisible()
  await expect(page.locator(`[data-operation-id="${first.operation_id}"] pre`)).not.toBeVisible()
  await page.locator(`[data-operation-id="${first.operation_id}"] summary`).filter({ hasText: /^Original command JSON$/ }).click()
  const original = JSON.parse((await page.locator(`[data-operation-id="${first.operation_id}"] pre`).textContent())!)
  expect(original.text).toBe(exact)
})


test('exact operation inclusion reconciles one outgoing message and leaves equal-text siblings queued', async ({ page, request }) => {
  await openFixture(page, request, { receipt: 'admitted', snapshot: actorSnapshot() })
  await chooseWorker(page)
  const input = page.getByLabel('Message to selected actor', { exact: true })
  for (let index = 0; index < 2; index++) {
    await input.fill('Equal text')
    await page.getByRole('button', { name: 'Send input', exact: true }).click()
    await expect.poll(async () => (await observations(request)).commands.length).toBe(index + 1)
  }
  const commands = (await observations(request)).commands
  await expect(page.locator('[data-local-operation-id]')).toHaveCount(2)
  const first = commands[0]
  await control(request, 'status', { operationId: first.operation_id, command: first.command, state: 'input_admitted', envelopeId: '1',
    receipt: { commandId: first.operation_id, target: first.command.target, outcome: 'admitted', envelopeId: '1' },
    inputObservation: { state: 'included', request: 'exact-inclusion-request' } })
  await expect(page.locator(`[data-local-operation-id="${first.operation_id}"]`)).toBeVisible()
  await expect.poll(async () => (await observations(request)).statusReads.filter((id: string) => id === first.operation_id).length).toBeGreaterThan(1)
  await control(request, 'config', { chatEntries: [message('1', { target: first.command.target, operationId: first.operation_id, envelopeId: '1' })] })
  await page.getByRole('button', { name: 'Refresh conversation', exact: true }).click()
  await expect(page.locator(`[data-local-operation-id="${first.operation_id}"]`)).toHaveCount(0)
  await expect(page.locator('[data-sequence="1"]')).toBeVisible()
  await expect(page.locator(`[data-local-operation-id="${commands[1].operation_id}"]`)).toBeVisible()
  expect((await observations(request)).commands).toHaveLength(2)
  await page.locator('summary').filter({ hasText: /^Host operations$/ }).click()
  const record = page.locator(`[data-operation-id="${first.operation_id}"]`)
  await expect(record).toContainText('exact-inclusion-request')
  await expect(record).toContainText('input_admitted')
})

const origin = { run: target.run, nativeActor: '7', incarnation: '1' }
function actorSnapshot() {
  const data = snapshot()
  data.actors = data.actors.map((actor, index) => index === 1 ? { ...actor, outputOrigin: origin, modelHeadRequest: 'request-worker-1' } : actor)
  return data
}
function message(sequence: string, inputOperation: Extract<ChatEntry, { kind: 'message' }>['inputOperation'], text = 'Equal text'): ChatEntry {
  return { sequence, kind: 'message', requestId: 'request-worker-1', position: sequence, hash: 'a'.repeat(64), tool: null, providerItemId: null, inputOperation,
    item: { type: 'message', role: 'user', content: text } }
}
