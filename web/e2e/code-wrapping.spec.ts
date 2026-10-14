import { test, expect } from '@playwright/test'
import type { ChatEntry } from '../src/generated/chat'
import { entries, message, openChat } from './chat-fixtures'
import { captureViewport } from './support'

const source = `let prompt =\n      "${'long_unbroken_Haskell_string_'.repeat(90)}"\n\nobserve prompt`

for (const width of [1280, 320]) {
  test(`chat code wraps long strings and copies exact source at ${width}px`, async ({ page, request, context }, info) => {
    await context.grantPermissions(['clipboard-read', 'clipboard-write'])
    await page.setViewportSize({ width, height: 844 })
    const chatEntries: ChatEntry[] = entries.map(entry => {
      if (entry.sequence === '1') return message('1', `\`\`\`haskell\n${source}\n\`\`\``)
      if (entry.kind === 'message' && entry.sequence === '2') return { ...entry, item: { type: 'custom_tool_call', call_id: 'cell', name: 'haskell', input: source } }
      if (entry.kind === 'message' && entry.sequence === '3') return { ...entry, item: { type: 'custom_tool_call_output', call_id: 'cell', output: source } }
      return entry
    })
    await openChat(page, request, chatEntries)
    const blocks = [page.locator('.message-code-block pre'), page.locator('.tool-source pre'), page.locator('.tool-results pre')]
    for (const block of blocks) {
      await expect(block).toHaveCount(1)
      const layout = await block.evaluate(element => {
        const style = getComputedStyle(element)
        return { width: element.clientWidth, scrollWidth: element.scrollWidth, whiteSpace: style.whiteSpace, wrap: style.overflowWrap }
      })
      expect(layout.whiteSpace).toBe('pre-wrap')
      expect(layout.wrap).toBe('anywhere')
      expect(layout.scrollWidth).toBeLessThanOrEqual(layout.width + 1)
      expect(await block.textContent()).toBe(source + (block === blocks[0] ? '\n' : ''))
    }
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(width)
    await page.locator('.message-code-block').getByRole('button', { name: /^Copy code from / }).click()
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source + '\n')
    await page.locator('.tool-source').getByRole('button', { name: 'Copy Input', exact: true }).click()
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(source)
    await captureViewport(page, info, `code-wrapping-${width}`)
  })
}
