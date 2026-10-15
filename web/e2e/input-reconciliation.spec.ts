import { test, expect } from '@playwright/test'
import { actorSnapshot, message } from './chat-fixtures'
import { chooseWorker, control, observations, openFixture, captureViewport } from './support'

for (const width of [1280, 390]) {
  test(`included inputs stay in retained chronology across paging, reconnect and replay at ${width}px`, async ({ page, request }, info) => {
    await page.setViewportSize({ width, height: 844 })
    const text = 'what next? any ideas? any suggestions?'
    const envelopeId = '9007199254740993'
    const replies = Array.from({ length: 70 }, (_, i) => message(String(i + 2), `Later Persona reply ${i + 1}`))
    await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: replies, receipt: 'admitted', envelopeId })
    await chooseWorker(page)
    const composer = page.getByRole('textbox', { name: /^Message / })
    await composer.fill(text)
    await composer.press('Control+Enter')
    await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
    const first = (await observations(request)).commands[0]
    await expect(page.getByLabel('outgoing message', { exact: true })).toHaveText(text)
    const canonical = { ...message('1', text), inputOperation: { target: first.command.target, operationId: first.operation_id, envelopeId },
      item: { type: 'message', role: 'user', content: text } }
    await control(request, 'config', { chatEntries: [canonical, ...replies] })
    await control(request, 'status', { operationId: first.operation_id, command: first.command, state: 'input_admitted', envelopeId,
      receipt: { commandId: first.operation_id, target: first.command.target, outcome: 'admitted', envelopeId },
      inputObservation: { state: 'included', request: 'request-worker-1' } })
    await expect(page.getByLabel('outgoing message', { exact: true })).toHaveCount(0)
    await expect(page.locator('.composer-delivery')).toHaveCount(0)
    await expect(page.getByText('Later Persona reply 70', { exact: true })).toBeVisible()
    await control(request, 'disconnect')
    await expect.poll(async () => (await observations(request)).connections).toBeGreaterThan(1)
    await page.reload()
    await expect(page.getByRole('button', { name: 'Load older messages' })).toBeVisible()
    await expect(page.getByLabel('outgoing message', { exact: true })).toHaveCount(0)
    await expect(page.locator('.composer-delivery')).toHaveCount(0)
    await captureViewport(page, info, `included-not-pending-${width}`)
    await page.getByRole('button', { name: 'Load older messages' }).click()
    await expect(page.getByLabel('Conversation entries').getByText(text, { exact: true })).toHaveCount(1)
    const order = await page.getByLabel('Conversation entries').getByRole('listitem').evaluateAll(elements => elements.map(element => element.textContent))
    expect(order.findIndex(value => value?.includes(text))).toBeLessThan(order.findIndex(value => value?.includes('Later Persona reply 1')))
    await control(request, 'config', { envelopeId: '9007199254740994' })
    await composer.fill(text)
    await composer.press('Control+Enter')
    await expect.poll(async () => (await observations(request)).commands.length).toBe(2)
    await expect(page.getByLabel('outgoing message', { exact: true })).toHaveText(text)
    await expect(page.getByLabel('Conversation entries').getByText(text, { exact: true })).toHaveCount(1)
    await page.locator('summary').filter({ hasText: /^Host operations$/ }).click()
    await expect(page.getByRole('region', { name: 'Retained browser operations' })).toContainText('Input included · request request-worker-1')
    await captureViewport(page, info, `distinct-identical-text-still-pending-${width}`)
    expect((await observations(request)).commands).toHaveLength(2)
    expect((await observations(request)).chatReads.some((read: { before: string | null }) => read.before !== null)).toBe(true)
  })
}
