import { afterEach, expect, it, vi } from 'vitest'
import { copyText } from './clipboard'

const clipboardDescriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
const commandDescriptor = Object.getOwnPropertyDescriptor(document, 'execCommand')
afterEach(() => {
  vi.restoreAllMocks()
  if (clipboardDescriptor) Object.defineProperty(navigator, 'clipboard', clipboardDescriptor)
  else Reflect.deleteProperty(navigator, 'clipboard')
  if (commandDescriptor) Object.defineProperty(document, 'execCommand', commandDescriptor)
  else Reflect.deleteProperty(document, 'execCommand')
  document.body.replaceChildren()
})

it.each([false, true])('falls back from an unavailable or rejected API and restores the active field (rejected: %s)', async rejected => {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: rejected ? { writeText: vi.fn().mockRejectedValue(new Error('refused')) } : undefined })
  const draft = document.createElement('textarea')
  draft.value = 'Retain this draft'
  document.body.append(draft)
  draft.focus()
  draft.setSelectionRange(2, 8, 'backward')
  const setData = vi.fn()
  Object.defineProperty(document, 'execCommand', { configurable: true, value: vi.fn(() => {
    const event = new Event('copy', { cancelable: true })
    Object.defineProperty(event, 'clipboardData', { value: { setData } })
    document.dispatchEvent(event)
    expect(event.defaultPrevented).toBe(true)
    return true
  }) })
  const text = '  λ 🐚\r\n\nkeep trailing spaces  '
  await copyText(text)
  expect(setData).toHaveBeenCalledExactlyOnceWith('text/plain', text)
  expect(document.activeElement).toBe(draft)
  expect([draft.selectionStart, draft.selectionEnd, draft.selectionDirection]).toEqual([2, 8, 'backward'])
  expect(draft.value).toBe('Retain this draft')
  expect(document.querySelectorAll('textarea')).toHaveLength(1)
  document.dispatchEvent(new Event('copy'))
  expect(setData).toHaveBeenCalledTimes(1)
})

it.each([false, 'throws'])('cleans up and restores document selection after copy refusal (%s)', async result => {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined })
  Object.defineProperty(document, 'execCommand', { configurable: true, value: () => {
    if (result === 'throws') throw new Error('denied')
    return false
  } })
  const button = document.createElement('button')
  button.textContent = 'Selected text'
  document.body.append(button)
  button.focus()
  const range = document.createRange()
  range.selectNodeContents(button)
  document.getSelection()!.removeAllRanges()
  document.getSelection()!.addRange(range)
  await expect(copyText('Source')).rejects.toThrow()
  expect(document.activeElement).toBe(button)
  expect(document.getSelection()?.toString()).toBe('Selected text')
  expect(document.querySelector('textarea')).toBeNull()
})
