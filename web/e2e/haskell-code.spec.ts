import { test, expect } from '@playwright/test'
import type { ChatEntry } from '../src/generated/chat'
import { actorSnapshot, message, tool } from './chat-fixtures'
import { chooseWorker, openFixture, noOverflow, captureViewport } from './support'

const source = `-- Haskell λ, real newlines, and literal escapes\nlet prompt = "literal \\n ${'long_unbroken_string_'.repeat(55)}"\n    values = [1, 2, 3]\n\nobserve (prompt, values)\n`
function invocation(sequence: string, position: string, call: string, item: unknown): ChatEntry {
  return { kind: 'message', sequence, requestId: 'issuer', position, hash: tool.invocation.hash, providerItemId: null, inputOperation: null,
    tool: { operation: { ...tool.operation, call }, invocation: { ...tool.invocation, position } }, item }
}
const argumentsText = JSON.stringify({ code: source })
const chatEntries: ChatEntry[] = [
  message('1', `\`\`\`haskell\n${source}\`\`\``),
  invocation('2', '0', 'custom', { type: 'custom_tool_call', name: 'haskell', call_id: 'custom', input: source }),
  invocation('3', '1', 'function', { type: 'function_call', name: 'haskell', call_id: 'function', arguments: argumentsText }),
  invocation('4', '2', 'other', { type: 'function_call', name: 'other', call_id: 'other', arguments: argumentsText }),
]

for (const width of [1280, 390]) {
  test(`Haskell custom and function bodies preserve source and highlight while wrapping at ${width}px`, async ({ page, request, context }, info) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    await page.setViewportSize({ width, height: 844 })
    await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries })
    await chooseWorker(page)
    const calls = page.getByRole('region', { name: 'haskell tool call', exact: true })
    await expect(calls).toHaveCount(2)
    for (const call of await calls.all()) {
      const code = call.locator('.tool-source code.language-haskell')
      await expect(code).toHaveCount(1)
      expect(await code.textContent()).toBe(source)
      const style = await code.evaluate(element => {
        const pre = element.parentElement!
        return { text: getComputedStyle(element).color, keyword: getComputedStyle(element.querySelector('.token.keyword')!).color,
          string: getComputedStyle(element.querySelector('.token.string')!).color, whiteSpace: getComputedStyle(pre).whiteSpace,
          width: pre.clientWidth, scrollWidth: pre.scrollWidth }
      })
      expect(style.keyword).not.toBe(style.text)
      expect(style.string).not.toBe(style.keyword)
      expect(style.whiteSpace).toBe('pre-wrap')
      expect(style.scrollWidth).toBeLessThanOrEqual(style.width + 1)
      await call.getByRole('button', { name: 'Copy Input', exact: true }).click()
      expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source)
    }
    const fence = page.locator('.message-code-block code.language-haskell')
    expect(await fence.textContent()).toBe(source)
    await expect(fence.locator('.token.keyword').first()).toBeAttached()
    await expect(page.getByRole('region', { name: 'other tool call', exact: true }).locator('.language-haskell')).toHaveCount(0)
    await noOverflow(page)
    await calls.first().evaluate(element => element.scrollIntoView({ block: 'start' }))
    await captureViewport(page, info, `haskell-code-${width}`)
  })
}
