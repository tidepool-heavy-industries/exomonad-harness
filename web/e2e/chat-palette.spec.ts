import { test, expect } from '@playwright/test'
import { actorSnapshot, entries } from './chat-fixtures'
import { audit, captureViewport, chooseWorker, control, observations, openFixture } from './support'

for (const palette of ['live-patina', 'solarized']) {
  for (const width of [1280, 390]) {
    test(`${palette} chat reading and response at ${width}px`, async ({ page, request }, info) => {
      await page.setViewportSize({ width, height: 844 })
      await openFixture(page, request, { snapshot: actorSnapshot(), chatEntries: entries })
      // Live Patina uses the actual default; Solarized is a review-only comparison.
      if (palette === 'solarized') await page.evaluate(() => { document.documentElement.dataset.palette = 'solarized' })
      await chooseWorker(page)
      const input = page.getByRole('textbox', { name: 'Message /root/worker', exact: true })
      await input.fill('Keep this draft while I review the tool result.')
      if (palette === 'live-patina') {
        await expect(page.locator('.composer-send')).toHaveCSS('background-color', 'rgb(68, 214, 192)')
        await expect(page.locator('.composer-send')).toHaveCSS('color', 'rgb(0, 31, 34)')
      }
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
      const typography = await tool.locator('.tool-source pre').first().evaluate(async element => {
        await document.fonts.ready
        const style = getComputedStyle(element)
        const canvas = document.createElement('canvas').getContext('2d')!
        canvas.font = `${style.fontSize} ${style.fontFamily}`
        return { family: style.fontFamily, size: style.fontSize, loaded: document.fonts.check('14px "IBM Plex Mono"'), narrow: canvas.measureText('iiiiiiii').width, wide: canvas.measureText('WWWWWWWW').width }
      })
      expect(typography.loaded).toBe(true)
      expect(typography.size).toBe('14px')
      expect(typography.narrow).toBeCloseTo(typography.wide, 1)
      await info.attach('code-typography', { body: JSON.stringify(typography), contentType: 'application/json' })
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
      if (palette === 'live-patina') {
        // Check actual filled controls and selection, not only root token names.
        const action = page.locator('.form-actions button[type="submit"]')
        const pairs = await page.evaluate(() => {
          const style = getComputedStyle(document.documentElement)
          return ['--accent', '--action-hover', '--selection'].map(role => ({
            background: style.getPropertyValue(role).trim(),
            foreground: style.getPropertyValue('--action-ink').trim(),
          }))
        })
        for (const pair of pairs) {
          const ratio = (hex: string) => {
            const c = hex.slice(1).match(/../g)!.map(x => parseInt(x, 16) / 255).map(v => v <= .04045 ? v / 12.92 : ((v + .055) / 1.055) ** 2.4)
            return c[0]! * .2126 + c[1]! * .7152 + c[2]! * .0722
          }
          expect((ratio(pair.background) + .05) / (ratio(pair.foreground) + .05)).toBeGreaterThanOrEqual(4.5)
        }
        const states: Record<string, unknown> = {}
        const appearance = () => action.evaluate(element => {
          const style = getComputedStyle(element)
          return { background: style.backgroundColor, ink: style.color, opacity: style.opacity }
        })
        await page.mouse.move(0, 0)
        states.default = await appearance()
        await expect(action).toHaveCSS('background-color', 'rgb(68, 214, 192)')
        await expect(action).toHaveCSS('color', 'rgb(0, 31, 34)')
        await action.hover()
        states.hover = await appearance()
        await expect(action).toHaveCSS('background-color', 'rgb(135, 228, 210)')
        await page.mouse.down()
        states.active = await appearance()
        await expect(action).toHaveCSS('background-color', 'rgb(134, 250, 227)')
        await expect(action).toHaveCSS('color', 'rgb(0, 31, 34)')
        // Release outside the button to avoid submitting the form.
        await page.mouse.move(0, 0)
        await page.mouse.up()
        await action.focus()
        await page.keyboard.press('Shift+Tab')
        await page.keyboard.press('Tab')
        await expect(action).toBeFocused()
        await expect(action).toHaveCSS('outline-style', 'solid')
        await captureViewport(page, info, 'form-focus')
        await info.attach('primary-action-states', { body: JSON.stringify(states), contentType: 'application/json' })
      }
      await control(request, 'config', { holdSnapshot: true })
      await control(request, 'disconnect')
      await expect(page.getByText('Disconnected · draft kept; reconnect to send.')).toBeVisible()
      await captureViewport(page, info, 'disconnected')
      if (width < 700) await page.getByRole('button', { name: /^Resume draft/ }).click()
      await expect(input).toHaveValue('Keep this draft while I review the tool result.')
      if (width < 700) await expect(input).toBeFocused()
      await expect(page.getByRole('button', { name: 'Send input', exact: true })).toBeDisabled()
      await expect(page.locator('.composer-send')).toHaveCSS('opacity', '0.6')
      expect((await observations(request)).commands).toHaveLength(0)
    })
  }
}
