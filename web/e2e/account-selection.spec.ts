import { test, expect } from '@playwright/test'
import { actorSnapshot, entries } from './chat-fixtures'
import { chooseWorker, control, observations, openFixture, noOverflow, captureViewport } from './support'

const selection = { selectedId: 'personal', accounts: [{ id: 'personal', label: 'Personal account' }, { id: 'team', label: 'Team account' }] }
function blockedSnapshot() {
  const data = actorSnapshot()
  return { ...data, accountSelection: selection, actors: data.actors.map((actor, index) => index === 1 ? { ...actor,
    modelHeadRequest: 'request-worker-1',
    inference: { state: 'blocked', block: { request: 'request-worker-1', generation: '1', reason: { kind: 'provider_stream_failure', event: 'response_failed', diagnostic: { code: 'subscription_sharing_usage_limit_exceeded', message: 'Provider quota response' } } } },
  } : actor) }
}

for (const width of [1280, 390]) {
  test(`run account selection survives reload and retries explicitly from a blocked child at ${width}px`, async ({ page, request }, info) => {
    await page.setViewportSize({ width, height: 844 })
    const snapshot = blockedSnapshot()
    await openFixture(page, request, { snapshot, chatEntries: entries, receipt: 'none' })
    await chooseWorker(page)
    if (width < 700) await page.getByRole('button', { name: 'Session options' }).click()
    const selector = page.getByRole('combobox', { name: 'Model account' })
    await expect(selector).toHaveValue('personal')
    await selector.focus()
    await expect(page.getByRole('button', { name: 'Review recovery options', exact: true })).toBeVisible()
    await selector.selectOption('team')
    expect((await observations(request)).commands).toHaveLength(0)
    await page.getByRole('button', { name: 'Apply account' }).click()
    await expect(selector).toBeDisabled()
    await expect(selector).toHaveValue('personal')
    await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
    const first = (await observations(request)).commands[0]
    expect(first.command).toEqual({ action: 'select_account', target: snapshot.actors[0]!.identity, account_id: 'team' })
    await page.reload()
    if (width < 700) await page.getByRole('button', { name: 'Session options' }).click()
    await expect(selector).toHaveValue('personal')
    await expect(selector).toBeDisabled()
    expect((await observations(request)).commands).toHaveLength(1)
    await control(request, 'config', { receipt: 'account_selected' })
    await page.getByRole('button', { name: 'Retry account selection' }).click()
    await expect(selector).toHaveValue('team')
    await expect(selector).toBeEnabled()
    const sent = (await observations(request)).commands
    expect(sent).toHaveLength(2)
    expect(sent[1]).toEqual(first)
    await page.getByRole('button', { name: 'Review recovery options', exact: true }).click()
    await expect(page.getByRole('button', { name: 'Authorize recovery' })).toBeDisabled()
    await noOverflow(page)
    await captureViewport(page, info, `account-selection-${width}`)
  })
}

test('refused account selection preserves the observed account and allows another explicit choice', async ({ page, request }) => {
  await openFixture(page, request, { snapshot: { ...actorSnapshot(), accountSelection: selection }, chatEntries: entries, receipt: 'refused' })
  const selector = page.getByRole('combobox', { name: 'Model account' })
  await selector.selectOption('team')
  await page.getByRole('button', { name: 'Apply account' }).click()
  await expect(selector).toBeEnabled()
  // A refusal never changes the authoritative account snapshot.
  await page.reload()
  await expect(selector).toHaveValue('personal')
  expect((await observations(request)).commands).toHaveLength(1)
})

test('opening the selector discovers a newly saved account without a restart or an inference command', async ({ page, request }) => {
  await openFixture(page, request, { snapshot: { ...actorSnapshot(), accountSelection: selection }, chatEntries: entries })
  const selector = page.getByRole('combobox', { name: 'Model account' })
  await expect(selector).toHaveValue('personal')
  await control(request, 'config', { accountInventory: { ...selection, accounts: [...selection.accounts, { id: 'new', label: 'Newly saved account' }] } })
  await selector.focus()
  await expect(selector.getByRole('option', { name: 'Newly saved account' })).toHaveCount(1)
  await expect(selector).toHaveValue('personal')
  expect((await observations(request)).commands).toHaveLength(0)
})
