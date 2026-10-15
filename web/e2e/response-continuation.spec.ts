import { test, expect } from '@playwright/test'
import { actorSnapshot, entries, modelOrigin } from './chat-fixtures'
import { chooseWorker, control, observations, openFixture, noOverflow, captureViewport } from './support'
const block = { request: 'request-worker-1', generation: '3', reason: { kind: 'continuity_unproven' } }
function interrupted(continuationRefusal?: 'exact_projection_unavailable' | 'uncertain_effects', generation = block.generation) {
  const data = actorSnapshot()
  return { ...data, actors: data.actors.map((actor, i) => i === 1 ? { ...actor, inference: { state: 'blocked', block: { ...block, generation, ...(continuationRefusal ? { continuationRefusal } : {}) } } } : actor) }
}
for (const width of [1280, 390]) {
  test(`Send queues text once and offers explicit context-loss confirmation at ${width}px`, async ({ page, request }, info) => {
    await page.setViewportSize({ width, height: 844 })
    const expectedBlock = { ...block, generation: '4', continuationRefusal: 'exact_projection_unavailable' }
    const snapshot = interrupted('exact_projection_unavailable', '4')
    await openFixture(page, request, { snapshot, chatEntries: entries, receipt: 'admitted', continuation: { state: 'refused', block: expectedBlock, reason: 'exact_projection_unavailable' } })
    await chooseWorker(page)
    const input = page.getByRole('textbox', { name: /^Message / })
    const text = 'Keep this queued λ\nwith exact whitespace '
    await input.fill(text)
    await input.press('Control+Enter')
    await expect(input).toHaveValue('')
    await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
    const commands = (await observations(request)).commands
    expect(commands[0].command).toEqual({ action: 'input', target: snapshot.actors[1]!.identity, text })
    expect(await page.getByLabel('outgoing message', { exact: true }).textContent()).toBe(text)
    await input.fill('My next draft stays here')
    await page.getByRole('button', { name: 'Review continuation' }).click()
    expect((await observations(request)).commands).toHaveLength(1)
    await expect(page.getByRole('group', { name: 'Continue without some reasoning context?' })).toBeFocused()
    await page.getByRole('button', { name: 'Cancel', exact: true }).click()
    await expect(page.getByRole('button', { name: 'Review continuation' })).toBeFocused()
    await expect(input).toHaveValue('My next draft stays here')
    await page.getByRole('button', { name: 'Review continuation' }).click()
    await noOverflow(page)
    await captureViewport(page, info, `continuation-confirmation-${width}`)
    await control(request, 'config', { receipt: 'none' })
    await page.getByRole('button', { name: 'Continue without this context' }).click()
    await expect.poll(async () => (await observations(request)).commands.length).toBe(2)
    const sent = (await observations(request)).commands
    expect(sent[1].command).toEqual({ action: 'recover', target: snapshot.actors[1]!.identity, expected_block: expectedBlock, mode: 'visible_history' })
    expect(sent.map((value: { command: { action: string } }) => value.command.action)).toEqual(['input', 'recover'])
    await expect(input).toHaveValue('My next draft stays here')
    expect(await page.getByLabel('outgoing message', { exact: true }).textContent()).toBe(text)
  })
}

test('host retry progress keeps the draft and uncertain tool outcomes cannot offer context fallback', async ({ page, request }) => {
  const data = actorSnapshot()
  const snapshot = { ...data, actors: data.actors.map((actor, i) => i === 1 ? { ...actor, inference: { state: 'auto_retry', block, attempt: 1, maximum: 2, delayMs: 1000 } } : actor) }
  await openFixture(page, request, { snapshot, chatEntries: entries })
  await chooseWorker(page)
  const input = page.getByRole('textbox', { name: /^Message / })
  await input.fill('Draft through retry')
  await input.evaluate((element: HTMLTextAreaElement) => element.setSelectionRange(2, 8))
  const reading = page.locator('.worker-chat-scroll')
  await reading.evaluate(element => { element.scrollTop = 100 })
  const position = await reading.evaluate(element => element.scrollTop)
  await expect(page.getByRole('button', { name: 'Retry', exact: true })).toHaveCount(0)
  expect((await observations(request)).commands).toHaveLength(0)
  await control(request, 'snapshot', { ...interrupted('exact_projection_unavailable'), seq: '20' })
  await page.getByRole('button', { name: 'Review continuation' }).click()
  await control(request, 'snapshot', { ...interrupted('uncertain_effects'), seq: '21' })
  await expect(page.getByRole('button', { name: 'Continue without this context' })).toHaveCount(0)
  await expect(page.getByRole('button', { name: 'Review continuation' })).toHaveCount(0)
  await expect(input).toHaveValue('Draft through retry')
  expect(await input.evaluate((element: HTMLTextAreaElement) => [element.selectionStart, element.selectionEnd])).toEqual([2, 8])
  expect(await reading.evaluate(element => element.scrollTop)).toBe(position)
  expect((await observations(request)).commands).toHaveLength(0)
})

test('interrupted partial tool arguments remain visible without becoming a completed call', async ({ page, request }) => {
  const snapshot = interrupted()
  const text = '{"code":"unfinished λ'
  await openFixture(page, request, { snapshot: { ...snapshot, requests: snapshot.requests.map(item => item.id === block.request ? { ...item, state: 'failed' } : item), liveOutput: [{
    origin: modelOrigin, requestId: block.request, itemId: 'partial', channel: 'tool_arguments', index: '0', text, version: '1', streaming: false, overflow: false, committedHash: null,
  }] }, chatEntries: [] })
  await chooseWorker(page)
  const preview = page.getByRole('region', { name: 'Tool call tool call' })
  await expect(preview.getByLabel('Arguments', { exact: true })).toHaveText(text)
  await expect(preview.getByText('Interrupted input preview')).toBeVisible()
  await preview.getByText('Tool evidence', { exact: true }).click()
  await expect(preview.getByText('Tool input preview; execution has not been established.')).toBeVisible()
  await expect(preview.getByRole('heading', { name: 'Result', exact: true })).toHaveCount(0)
  expect((await observations(request)).commands).toHaveLength(0)
})

test('Retry after reload resends the exact unresolved operation rather than creating a new decision', async ({ page, request }) => {
  await openFixture(page, request, { snapshot: interrupted(), chatEntries: entries, receipt: 'none' })
  await chooseWorker(page)
  await page.getByRole('button', { name: 'Retry', exact: true }).click()
  await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
  const first = (await observations(request)).commands[0]
  await page.reload()
  await page.getByRole('button', { name: 'Retry', exact: true }).click()
  await expect.poll(async () => (await observations(request)).commands.length).toBe(2)
  expect((await observations(request)).commands[1]).toEqual(first)
})
