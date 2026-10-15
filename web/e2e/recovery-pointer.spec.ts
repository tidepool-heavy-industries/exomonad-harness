import { test, expect } from '@playwright/test'
import { actorSnapshot, entries } from './chat-fixtures'
import { openFixture, chooseWorker, observations } from './support'

for (const draft of ['empty', 'focused', 'draft']) {
  test(`narrow Retry submits on the first pointer activation with ${draft} composer`, async ({ page, request }) => {
    await page.setViewportSize({ width: 390, height: 844 })
    const data = actorSnapshot()
    const snapshot = { ...data, actors: data.actors.map((actor, index) => index === 1 ? { ...actor,
      modelHeadRequest: 'request-worker-1', inference: { state: 'blocked', block: { request: 'request-worker-1', generation: '1', reason: { kind: 'continuity_unproven' } } },
    } : actor) }
    await openFixture(page, request, { snapshot, chatEntries: entries })
    await chooseWorker(page)
    const composer = page.getByRole('textbox', { name: /^Message / })
    if (draft === 'focused') await composer.focus()
    if (draft === 'draft') await composer.fill('Keep this draft λ')
    await page.getByRole('button', { name: 'Retry', exact: true }).click()
    await expect.poll(async () => (await observations(request)).commands.length).toBe(1)
    expect((await observations(request)).commands[0].command).toMatchObject({ action: 'recover', mode: 'exact', expected_block: { request: 'request-worker-1', generation: '1', reason: { kind: 'continuity_unproven' } } })
    await expect(composer).toHaveValue(draft === 'draft' ? 'Keep this draft λ' : '')
    expect((await observations(request)).commands).toHaveLength(1)
  })
}
