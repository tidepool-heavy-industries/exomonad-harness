import { act, fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { refractor } from 'refractor/core'
import { MessageContent } from './MessagePresentation'
import ToolActivity from './ToolActivity'
import type { LiveOutput } from './live-output'

afterEach(() => { vi.useRealTimers(); vi.restoreAllMocks() })

const item: LiveOutput = { origin: { kind: 'embedded', run: 'run', actor: '/root', incarnation: 'one' }, requestId: 'request',
  itemId: 'item', channel: 'tool_input', toolName: 'haskell', index: '0', version: '1', text: '', streaming: true, overflow: false, committedHash: null }
const source = '-- λ 🐈\nlet prompt = "literal \\n and \\"quote\\""\n    value = 42\n{- nested {- comment -} here -}\nobserve prompt\n'
function tool(text: string, channel: LiveOutput['channel'] = 'tool_input', streaming = true, toolName = 'haskell') {
  return <ToolActivity row={{ kind: 'live', key: 'live', live: [{ ...item, text, channel, streaming, toolName }] }} streamingActive={streaming} />
}

it.each(['haskell', 'haskell_sync'])('highlights %s custom source through partial strings/comments and completion without altering text', name => {
  vi.useFakeTimers()
  const { container, rerender } = render(tool('', 'tool_input', true, name))
  for (const end of [4, 10, 27, 45, 78, source.length]) {
    const prefix = source.slice(0, end)
    rerender(tool(prefix, 'tool_input', true, name))
    act(() => vi.advanceTimersByTime(80))
    expect(container.querySelector('code.language-haskell')?.textContent).toBe(prefix)
  }
  expect(container.querySelector('.token.keyword')).not.toBeNull()
  expect(container.querySelector('.token.string')).not.toBeNull()
  expect(container.querySelector('.token.comment')).not.toBeNull()
  rerender(tool(source + '-- completed\n', 'tool_input', false, name))
  expect(container.querySelector('code')?.textContent).toBe(source + '-- completed\n')
})

it('keeps unfinished named JSON arguments exact and plain until JSON parsing identifies source', () => {
  vi.useFakeTimers()
  const json = JSON.stringify({ code: source })
  const { container, rerender } = render(tool('', 'tool_arguments'))
  for (const end of [9, 25, json.length - 1]) {
    const prefix = json.slice(0, end)
    rerender(tool(prefix, 'tool_arguments'))
    act(() => vi.advanceTimersByTime(80))
    expect(screen.getByLabelText('Arguments').textContent).toBe(prefix)
    expect(container.querySelector('code.language-haskell')).toBeNull()
  }
  rerender(tool(json, 'tool_arguments'))
  act(() => vi.advanceTimersByTime(80))
  expect(container.querySelector('code.language-haskell')?.textContent).toBe(source)
  rerender(tool(json, 'tool_arguments', false))
  expect(container.querySelector('code')?.textContent).toBe(source)
})

it('renders an open Haskell fence progressively and preserves its completed source', () => {
  vi.useFakeTimers()
  const { container, rerender } = render(<MessageContent text="```ha" label="Answer" format="markdown" streaming />)
  for (const end of [10, 27, 78, source.length]) {
    const prefix = source.slice(0, end)
    rerender(<MessageContent text={'```haskell\n' + prefix} label="Answer" format="markdown" streaming />)
    act(() => vi.advanceTimersByTime(80))
    // Markdown owns the conventional final newline of a rendered code fence.
    expect(container.querySelector('code.language-haskell')?.textContent).toBe(prefix.endsWith('\n') ? prefix : prefix + '\n')
  }
  rerender(<MessageContent text={'```haskell\n' + source + '```'} label="Answer" format="markdown" />)
  expect(container.querySelector('code')?.textContent).toBe(source)
  expect(container.querySelector('.token.keyword')).not.toBeNull()
})

it('bounds tokenization by display cadence and copies latest exact text between frames', async () => {
  vi.useFakeTimers()
  const highlight = vi.spyOn(refractor, 'highlight')
  const writeText = vi.fn().mockResolvedValue(undefined)
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  try {
    const { container, rerender } = render(tool('let value = "'))
    const baseline = highlight.mock.calls.length
    for (let i = 1; i <= 1000; i++) rerender(tool('let value = "' + 'λ'.repeat(i)))
    expect(highlight.mock.calls.length).toBe(baseline)
    fireEvent.click(screen.getByRole('button', { name: 'Copy Input' }))
    expect(writeText).toHaveBeenLastCalledWith('let value = "' + 'λ'.repeat(1000))
    act(() => vi.advanceTimersByTime(80))
    expect(highlight.mock.calls.length).toBe(baseline + 1)
    expect(container.querySelector('code')?.textContent).toBe('let value = "' + 'λ'.repeat(1000))
    rerender(tool('let value = "' + 'λ'.repeat(20000) + '"\n', 'tool_input', false))
    expect(container.querySelector('code')?.textContent).toBe('let value = "' + 'λ'.repeat(20000) + '"\n')
  } finally {
    if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor)
    else Reflect.deleteProperty(navigator, 'clipboard')
  }
})
