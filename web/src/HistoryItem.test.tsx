import { describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import HistoryItem from './HistoryItem'

const entry = (item: unknown) => ({ position: '7', hash: 'a'.repeat(64), byteLen: '123', item })
const exact = '  λ 🐈\n"quoted" \\  <script>alert(1)</script>  '

function showRaw() {
  fireEvent.click(screen.getByText('Message details'))
  fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
}

describe('retained Item presentation', () => {
  it.each([{ summary: [] }, { summary: [{ type: 'summary_text', text: '   \n' }] }])('omits opaque reasoning without a nonempty readable summary: %j', ({ summary }) => {
    const { container } = render(<HistoryItem entry={entry({ type: 'reasoning', summary, encrypted_content: 'opaque' })} />)
    expect(container.textContent).toBe('')
  })

  it('omits opaque-only reasoning with no summary field', () => {
    const { container } = render(<HistoryItem entry={entry({ type: 'reasoning', encrypted_content: 'opaque' })} />)
    expect(container.textContent).toBe('')
  })

  it.each([
    { type: 'reasoning', summary: [{ type: 'future_summary', payload: 'λ 🐈' }], encrypted_content: 'opaque' },
    { type: 'reasoning', summary: { future: 'summary shape' } },
    { type: 'reasoning', summary: [], content: [{ type: 'future_reasoning', payload: 'λ 🐈' }] },
    { type: 'reasoning', content: { future: 'content shape' } },
  ])('preserves unfamiliar reasoning blocks and shapes for exact raw inspection: %j', item => {
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByText(/Unknown content is retained/)).toBeVisible()
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('renders assistant Markdown and preserves the entire original wire item for inspection', () => {
    const item = { type: 'message', role: 'assistant', phase: 'commentary', content: '# Result\n\n**Ready** with `code`.\n\n<script>alert(1)</script>' }
    const { container } = render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByRole('heading', { name: 'Assistant · commentary' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Result' })).toBeInTheDocument()
    expect(container.querySelector('strong')?.textContent).toBe('Ready')
    expect(container.querySelector('script')).toBeNull()
    expect(container.querySelector('.chat-message-assistant')).toBeInTheDocument()
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('keeps human text exact and discloses unknown blocks in Raw', () => {
    const item = { type: 'message', role: 'user', content: [
      { type: 'input_text', text: exact }, { type: 'output_text', text: ' second\n' },
      { type: 'future_media', bytes: 'unknown-payload' },
    ] }
    const { container } = render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Text item 7 block 1').textContent).toBe(exact)
    expect(screen.getByLabelText('Text item 7 block 2').textContent).toBe(' second\n')
    expect(container.querySelector('.chat-message-human')).toBeInTheDocument()
    expect(container.querySelector('script')).toBeNull()
    expect(screen.getByText(/Unknown content is retained/)).toBeInTheDocument()
    expect(screen.queryByLabelText('Raw item 7')).toBeNull()
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    fireEvent.click(screen.getByRole('button', { name: 'Hide Raw item 7' }))
    expect(screen.queryByLabelText('Raw item 7')).toBeNull()
  })

  it('folds nonempty reasoning summaries independently of exact metadata and raw inspection', () => {
    const item = { type: 'reasoning', summary: [{ type: 'summary_text', text: '**Summary**\n\nSecond paragraph.' }], encrypted_content: 'opaque' }
    const { container } = render(<HistoryItem entry={entry(item)} />)
    const fold = container.querySelector('.chat-message-fold')!
    expect(fold).not.toHaveAttribute('open')
    expect(screen.getByLabelText('Summary item 7')).not.toBeVisible()
    fireEvent.click(screen.getByText('Reasoning summary'))
    expect(fold).toHaveAttribute('open')
    expect(screen.getByLabelText('Summary item 7')).toBeVisible()
    expect(screen.queryByText('opaque')).toBeNull()
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it.each([
    [{ type: 'reasoning', summary: [], content: [{ type: 'reasoning_text', text: exact }] }, 'Reasoning'],
    [{ type: 'function_call', name: 'f', call_id: 'call λ', arguments: 'arguments as exact text' }, 'Arguments'],
    [{ type: 'custom_tool_call', name: 'cell', call_id: 'call λ', input: exact }, 'Input'],
    [{ type: 'function_call_output', call_id: 'call λ', output: 'Tool failed: permission denied\n' }, 'Result'],
    [{ type: 'custom_tool_call_output', call_id: 'call λ', output: exact }, 'Result'],
    [{ type: 'configuration_update', reasoning: { effort: 'high' } }, 'Reasoning effort'],
  ] as const)('retains original plain text for %j', (item, label) => {
    render(<HistoryItem entry={entry(item)} />)
    const source = 'content' in item ? exact : 'arguments' in item ? item.arguments : 'input' in item ? item.input
      : 'output' in item ? item.output : item.reasoning.effort
    expect(screen.getByLabelText(`${label} item 7`).textContent).toBe(source)
    if (item.type === 'function_call' || item.type === 'custom_tool_call') expect(screen.getByRole('heading').textContent).toContain('Model tool call')
    if (item.type === 'function_call_output' || item.type === 'custom_tool_call_output') expect(screen.getByRole('heading', { name: 'Tool response' })).toBeInTheDocument()
  })

  it('renders a refusal as assistant content and retains its original payload', () => {
    const item = { type: 'message', role: 'assistant', content: [{ type: 'refusal', refusal: '**Unable** to continue.' }] }
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Text item 7').querySelector('strong')?.textContent).toBe('Unable')
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it.each([null, false, 0, { type: 'future_item', extra: ['λ'] }, { type: 'custom_tool_call_output', call_id: 'c', output: false }, { type: 'custom_tool_call', call_id: 'c', name: 'f', input: { z: 1 } }])('shows arbitrary or unfamiliar JSON %j', item => {
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    expect(screen.queryByText(/success/i)).toBeNull()
  })

  it('renders object function arguments as structured JSON with exact call metadata', () => {
    const argumentsObject = { z: 1, nested: { unicode: '  λ\n🐈  ', nullable: null }, choices: [false, 0] }
    const item = { type: 'function_call', call_id: 'call λ', name: 'structured', arguments: argumentsObject }
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByRole('heading', { name: 'Model tool call · structured' })).toBeInTheDocument()
    expect(screen.getByLabelText('Arguments JSON item 7').textContent).toBe(JSON.stringify(argumentsObject, null, 2))
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('formats raw JSON lazily and keeps item metadata folded', () => {
    const item = { type: 'message', role: 'user', content: exact }
    const serialize = vi.spyOn(JSON, 'stringify')
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByText(/Item 7 · hash/)).not.toBeVisible()
    expect(serialize.mock.calls.some(([value]) => value === item)).toBe(false)
    showRaw()
    expect(screen.getByText(/Item 7 · hash/)).toBeVisible()
    expect(serialize.mock.calls.some(([value]) => value === item)).toBe(true)
    serialize.mockRestore()
  })

  it.each(['function_call', 'custom_tool_call', 'function_call_output', 'custom_tool_call_output'])('copies exact retained source for formatted %s', async type => {
    const source = ' { "z": 1, "cat" : "λ 🐈" }\n'
    const item = type === 'function_call' ? { type, name: 'tool', call_id: 'c', arguments: source }
      : type === 'custom_tool_call' ? { type, name: 'cell', call_id: 'c', input: source }
      : { type, call_id: 'c', output: source }
    const label = type === 'function_call' ? 'Arguments' : type === 'custom_tool_call' ? 'Input' : 'Result'
    const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    try {
      render(<HistoryItem entry={entry(item)} />)
      if (label === 'Result') fireEvent.click(screen.getByText(/^JSON result/))
      expect(screen.getByLabelText(`${label} item 7`).textContent).toBe(JSON.stringify(JSON.parse(source), null, 2))
      fireEvent.click(screen.getByRole('button', { name: `Copy ${label} item 7` }))
      await waitFor(() => expect(writeText).toHaveBeenCalledWith(source))
      showRaw()
      expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    } finally {
      if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor)
      else Reflect.deleteProperty(navigator, 'clipboard')
    }
  })

  it('retains long Unicode text and raw source without a character cut', () => {
    const text = 'x'.repeat(1999) + '🐈 λ\n' + 'α'.repeat(5000) + '\n original tail'
    const item = { type: 'message', role: 'user', content: text }
    render(<HistoryItem entry={entry(item)} listItem={false} />)
    expect(screen.queryByRole('listitem')).toBeNull()
    expect(screen.getByLabelText('Text item 7').textContent).toBe(text)
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('keeps unknown textless reasoning inspectable through Raw', () => {
    const item = { type: 'reasoning', summary: [{ type: 'future_summary', value: 'opaque summary' }], encrypted_content: 'opaque' }
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByRole('heading', { name: 'Reasoning' })).toBeInTheDocument()
    expect(screen.getByText(/Unknown content is retained/)).toBeInTheDocument()
    expect(screen.queryByText(/show readable text/i)).toBeNull()
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })
  it('labels model calls separately and keeps transport identifiers in closed metadata', () => {
    const item = { type: 'function_call', id: 'fc-item-1', call_id: 'call-1', name: 'read_file', arguments: '{"path":"README","limit":3}' }
    render(<HistoryItem entry={entry(item)} />)
    const heading = screen.getByRole('heading', { name: 'Model tool call · read_file' })
    expect(heading).not.toHaveTextContent('call-1')
    expect(screen.getByLabelText('Arguments item 7').textContent).toBe('{\n  "path": "README",\n  "limit": 3\n}')
    expect(screen.getByLabelText('Arguments item 7').closest('.history-code')).toBeInTheDocument()
    const details = screen.getByText('Message details').closest('details')!
    expect(details).not.toHaveAttribute('open')
    fireEvent.click(screen.getByText('Message details'))
    expect(details).toHaveTextContent('Item ID: fc-item-1')
    expect(details).toHaveTextContent('Call ID: call-1')
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('labels tool responses and collapses formatted JSON behind a concise preview', () => {
    const response = {
      ready_results: [{ call: 'call-CN', origin: { actor: '/root', incarnation: '1', kind: 'embedded', run: 'run-1' }, request: 'request-1' }],
      reason: 'tool_result',
    }
    const compact = JSON.stringify(response)
    const item = { type: 'function_call_output', id: 'fc-output-1', call_id: 'call-CN', output: compact }
    render(<HistoryItem entry={entry(item)} />)
    const heading = screen.getByRole('heading', { name: 'Tool response' })
    expect(heading).not.toHaveTextContent('call-CN')
    const disclosure = screen.getByText('JSON result · 2 fields · reason: tool_result')
    expect(disclosure.tagName).toBe('SUMMARY')
    expect(disclosure.closest('details')).not.toHaveAttribute('open')
    fireEvent.click(disclosure)
    expect(screen.getByLabelText('Result item 7').textContent).toBe(JSON.stringify(response, null, 2))
    expect(screen.getByLabelText('Result item 7').closest('.history-code')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Message details'))
    expect(screen.getByText('Message details').closest('details')).toHaveTextContent('Call ID: call-CN')
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it.each([
    ['nested JSON', JSON.stringify({ ready_results: [{ origin: { actor: '/root' } }] }), 'JSON result · 1 field'],
    ['array JSON', JSON.stringify([{ call: 'call-1', origin: { actor: '/root' } }]), 'JSON result · 1 item'],
    ['long-key JSON', JSON.stringify({ [`${'a'.repeat(60)}`]: 'short' }), `JSON result · 1 field · ${'a'.repeat(45)}…: short`],
  ])('collapses %s with a bounded preview and keeps the formatted response available', (_name, output, summary) => {
    render(<HistoryItem entry={entry({ type: 'function_call_output', call_id: 'call-1', output })} />)
    const disclosure = screen.getByText(summary)
    expect(disclosure.tagName).toBe('SUMMARY')
    expect(disclosure.closest('details')).not.toHaveAttribute('open')
    fireEvent.click(disclosure)
    expect(screen.getByLabelText('Result item 7').textContent).toBe(JSON.stringify(JSON.parse(output), null, 2))
  })

  it('labels final assistant text and exposes unknown phases through Raw', () => {
    const finalItem = { type: 'message', role: 'assistant', phase: 'final', content: 'Done.' }
    const { rerender } = render(<HistoryItem entry={entry(finalItem)} />)
    expect(screen.getByRole('heading', { name: 'Assistant · final' })).toBeInTheDocument()
    const futureItem = { type: 'message', role: 'assistant', phase: 'future_phase', content: 'Retained.' }
    rerender(<HistoryItem entry={entry(futureItem)} />)
    expect(screen.getByRole('heading', { name: 'Assistant · unknown phase (future_phase)' })).toBeInTheDocument()
    expect(screen.getByText(/Unknown content is retained/)).toBeInTheDocument()
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(futureItem, null, 2))
  })


})
