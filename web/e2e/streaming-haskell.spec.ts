import { test, expect } from '@playwright/test'
import { actorSnapshot, modelOrigin, message, tool } from './chat-fixtures'
import { chooseWorker, openFixture, noOverflow, captureViewport, control } from './support'

const source = `-- λ 🐈 exact source\nlet prompt = "literal \\n ${'long_unbroken_string_'.repeat(70)}"\n    values = [1, 2, 3]\n{- nested {- comment -} still open`
for (const width of [1280, 390]) {
  test(`streamed Haskell source and open fences highlight, wrap and copy before completion at ${width}px`, async ({ page, request, context }, info) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    await page.setViewportSize({ width, height: 844 })
    await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: [] })
    await chooseWorker(page)
    let seq = 10
    async function delta(itemId: string, channel: string, text: string, toolName?: string) {
      await control(request, 'frame', { type: 'event', event: { seq: String(++seq), event: { kind: 'model.output.delta', value: {
        origin: modelOrigin, requestId: 'request-worker-1', itemId, channel, index: '0', version: String(seq), text, overflow: false,
        ...(toolName ? { toolName } : {}),
      } } } })
    }
    const calls = page.getByRole('region', { name: 'haskell tool call', exact: true })
    await delta('custom', 'tool_input', source.slice(0, 30), 'haskell')
    await expect(calls.locator('code.language-haskell')).toHaveText(source.slice(0, 30), { useInnerText: false })
    await delta('custom', 'tool_input', source.slice(30), 'haskell')
    await expect.poll(() => calls.locator('code').textContent()).toBe(source)
    await delta('function', 'tool_arguments', '{"code":', 'haskell_sync')
    const named = page.getByRole('region', { name: 'haskell_sync tool call', exact: true })
    await expect(named.locator('.message-content-body')).toHaveText('{"code":')
    await expect(named.locator('code.language-haskell')).toHaveCount(0)
    await delta('function', 'tool_arguments', JSON.stringify(source) + '}', 'haskell_sync')
    await expect.poll(() => named.locator('code').textContent()).toBe(source)
    await delta('fence', 'assistant', '```ha')
    await delta('fence', 'assistant', 'skell\n' + source)
    const fence = page.locator('.message-code-block code.language-haskell')
    await expect.poll(() => fence.textContent()).toBe(source + '\n')
    await expect(fence.locator('.token.keyword').first()).toBeAttached()
    for (const call of [calls, named]) {
      const code = call.locator('code.language-haskell')
      await expect(code.locator('.token.keyword').first()).toBeAttached()
      const dimensions = await code.evaluate(element => {
        const pre = element.parentElement!
        return { width: pre.clientWidth, scrollWidth: pre.scrollWidth, whiteSpace: getComputedStyle(pre).whiteSpace }
      })
      expect(dimensions.whiteSpace).toBe('pre-wrap')
      expect(dimensions.scrollWidth).toBeLessThanOrEqual(dimensions.width + 1)
      await call.getByRole('button', { name: 'Copy Input', exact: true }).click()
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source)
    }
    await noOverflow(page)
    await calls.evaluate(element => element.scrollIntoView({ block: 'start' }))
    await captureViewport(page, info, `streaming-haskell-${width}`)
    const completed = source + ' -}\nobserve prompt\n'
    await delta('custom', 'tool_input', ' -}\nobserve prompt\n', 'haskell')
    await expect.poll(() => calls.locator('code').textContent()).toBe(completed)
    await control(request, 'config', { chatEntries: [message('1', '```haskell\n' + completed + '```', 'fence'), {
      kind: 'message', sequence: '2', requestId: 'request-worker-1', position: '2', hash: 'a'.repeat(64), providerItemId: 'custom', inputOperation: null,
      tool: { ...tool, operation: { ...tool.operation, request: 'request-worker-1', call: 'custom' }, invocation: { ...tool.invocation, request: 'request-worker-1', position: '2' } },
      item: { type: 'custom_tool_call', id: 'custom', name: 'haskell', call_id: 'custom', input: completed },
    }] })
    await expect(page.locator('[data-sequence="2"]')).toBeVisible()
    const retained = page.locator('[data-sequence="2"] code.language-haskell')
    expect(await retained.textContent()).toBe(completed)
    await expect(retained.locator('.token.string')).toBeAttached()
    await noOverflow(page)
  })
}
