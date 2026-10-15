import { test, expect, chromium } from '@playwright/test'
import type { ChatEntry } from '../src/generated/chat'
import { actorSnapshot, entries, message } from './chat-fixtures'
import { chooseWorker, control } from './support'

test('HTTP chat copies exact source with keyboard and preserves selection and draft', async ({ request }) => {
  const source = '  let prompt = "λ 🐚"\n\n      observe prompt  \n'
  const chatEntries: ChatEntry[] = entries.map(entry => {
    if (entry.sequence === '1') return message('1', `\`\`\`haskell\n${source}\`\`\``)
    if (entry.kind === 'message' && entry.sequence === '2') return { ...entry, item: { type: 'custom_tool_call', call_id: 'cell', name: 'haskell', input: source } }
    return entry
  })
  await control(request, 'reset', { snapshot: actorSnapshot(), chatEntries })
  // A DNS name mapped to loopback remains an insecure HTTP origin, unlike localhost.
  const browser = await chromium.launch({ executablePath: chromium.executablePath(), args: ['--host-resolver-rules=MAP harness-http.test 127.0.0.1', '--no-proxy-server'] })
  try {
    const page = await browser.newPage()
    await page.goto('http://harness-http.test:4387/')
    await expect(page.getByRole('heading', { name: 'Tree', exact: true })).toBeVisible()
    await chooseWorker(page)
    expect(await page.evaluate(() => ({ secure: window.isSecureContext, clipboard: typeof navigator.clipboard }))).toEqual({ secure: false, clipboard: 'undefined' })
    const composer = page.getByRole('textbox', { name: /^Message / })
    await composer.fill('Keep this draft')
    const code = page.locator('.message-content-markdown .message-code-block pre code')
    await code.evaluate(element => {
      const text = element.firstChild!
      document.getSelection()!.setBaseAndExtent(text, text.textContent!.length, text, 0)
    })
    const copyCode = page.locator('.message-content-markdown .message-code-block').getByRole('button', { name: /^Copy code from / })
    await copyCode.focus()
    const selectedText = await page.evaluate(() => document.getSelection()?.toString())
    await page.keyboard.press('Enter')
    await expect(copyCode).toBeFocused()
    await expect(page.getByRole('status').filter({ hasText: 'Copied to clipboard.' })).toHaveCount(1)
    expect(await page.evaluate(() => document.getSelection()?.toString())).toBe(selectedText)
    expect(await page.evaluate(() => document.getSelection()!.anchorOffset > document.getSelection()!.focusOffset)).toBe(true)
    await expect(composer).toHaveValue('Keep this draft')
    await composer.focus()
    await page.keyboard.press('Control+A')
    await page.keyboard.press('Control+V')
    await expect(composer).toHaveValue(source)
    const copyInput = page.locator('.tool-source').getByRole('button', { name: 'Copy Input', exact: true })
    await copyInput.click()
    await expect(copyInput).toBeFocused()
    await expect(page.getByRole('status').filter({ hasText: 'Copied to clipboard.' })).toHaveCount(2)
    await composer.focus()
    await page.keyboard.press('Control+A')
    await page.keyboard.press('Control+V')
    await expect(composer).toHaveValue(source)
    await expect(page.locator('body > textarea')).toHaveCount(0)
  } finally { await browser.close() }
})
