import { test, expect } from '@playwright/test'
import { actorSnapshot, entries } from './chat-fixtures'
import { audit, captureViewport, chooseWorker, control, observations, openFixture } from './support'

for (const palette of ['catppuccin-mocha', 'solarized']) {
  for (const width of [1280, 390]) {
    test(`${palette} chat reading and response at ${width}px`, async ({ page, request }, info) => {
      await page.setViewportSize({ width, height: 844 })
      await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: entries })
      // Mocha uses the actual default; Solarized is a review-only comparison.
      if (palette === 'solarized') await page.evaluate(() => { document.documentElement.dataset.palette = 'solarized' })
      await chooseWorker(page)
      const input = page.getByRole('textbox', { name: 'Message /root/worker', exact: true })
      await input.fill('Keep this draft while I review the tool result.')
      if (width < 700) {
        const before = (await page.getByRole('region', { name: 'Worker conversation', exact: true }).boundingBox())!.height
        await page.getByRole('button', { name: 'Read conversation', exact: true }).click()
        await expect(page.getByRole('button', { name: /^Resume draft/ })).toBeFocused()
        await expect(input).not.toBeVisible()
        const after = (await page.getByRole('region', { name: 'Worker conversation', exact: true }).boundingBox())!.height
        expect(after - before).toBeGreaterThan(80)
        await info.attach('reading-space', { body: JSON.stringify({ before, after, gained: after - before }), contentType: 'application/json' })
      }
      await page.locator('[data-sequence="2"]').evaluate(element => element.scrollIntoView({ block: 'start' }))
      const tool = page.getByRole('region', { name: 'haskell tool call' })
      await expect(tool.locator('.tool-result-excerpt')).toContainText('Compilation failed')
      await captureViewport(page, info, 'tool-summary')
      await tool.getByText('Tool evidence', { exact: true }).click()
      await expect(tool.getByText('Execution log', { exact: true })).toBeVisible()
      await tool.getByRole('button', { name: 'Show raw result item', exact: true }).click()
      await expect(tool.getByLabel('Raw result item', { exact: true })).toContainText('runCheck')
      await tool.getByText('Tool evidence', { exact: true }).click()
      await page.getByRole('button', { name: 'Jump to form', exact: true }).click()
      await expect(page.getByRole('textbox', { name: 'Project name', exact: true })).toBeFocused()
      await audit(page, info, 'pending-form')
      const contrast = await page.evaluate(() => {
        const style = getComputedStyle(document.documentElement)
        const luminance = (hex: string) => {
          const channels = hex.trim().slice(1).match(/../g)!.map(x => parseInt(x, 16) / 255).map(c => c <= .04045 ? c / 12.92 : ((c + .055) / 1.055) ** 2.4)
          return channels[0]! * .2126 + channels[1]! * .7152 + channels[2]! * .0722
        }
        return ['--paper', '--panel'].flatMap(background => ['--ink', '--muted', '--link', '--failed', '--pending-text', '--waiting', '--control-border', '--accent'].map(role => {
          const a = luminance(style.getPropertyValue(role)), b = luminance(style.getPropertyValue(background))
          return { background, role, ratio: (Math.max(a, b) + .05) / (Math.min(a, b) + .05) }
        }))
      })
      for (const pair of contrast) expect(pair.ratio, `${pair.role} on ${pair.background}`).toBeGreaterThanOrEqual(['--control-border', '--accent'].includes(pair.role) ? 3 : 4.5)
      await info.attach('palette-contrast', { body: JSON.stringify(contrast), contentType: 'application/json' })
      await control(request, 'config', { holdSnapshot: true })
      await control(request, 'disconnect')
      await expect(page.getByText('Disconnected · draft kept; reconnect to send.')).toBeVisible()
      await captureViewport(page, info, 'disconnected')
      if (width < 700) await page.getByRole('button', { name: /^Resume draft/ }).click()
      await expect(input).toHaveValue('Keep this draft while I review the tool result.')
      if (width < 700) await expect(input).toBeFocused()
      await expect(page.getByRole('button', { name: 'Send input', exact: true })).toBeDisabled()
      expect((await observations(request)).commands).toHaveLength(0)
    })
  }
}
