import { describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen } from '@testing-library/react'
import HistoryItem from './HistoryItem'

const entry = (item: unknown) => ({ position: 7, hash: 'a'.repeat(64), byteLen: 123, item })
const exact = '  λ 🐈\n"quoted" \\  <script>alert(1)</script>  '

describe('retained Item presentation', () => {
  it('omits opaque reasoning items without a readable summary', () => {
    const {container} = render(<HistoryItem entry={entry({type:'reasoning', summary:[], encrypted_content:'opaque'})} />)
    expect(container.textContent).toBe('')
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
    expect(screen.getByLabelText('Arguments item 7')).toHaveClass('history-code')
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
    expect(screen.getByLabelText('Result item 7')).toHaveClass('history-code')
    fireEvent.click(screen.getByText('Message details'))
    expect(screen.getByText('Message details').closest('details')).toHaveTextContent('Call ID: call-CN')
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })
  it('shows a readable role and exact escaped string content without executing HTML', () => {
    const { container } = render(<HistoryItem entry={entry({ type: 'message', role: 'assistant', phase: 'commentary', content: exact })} />)
    expect(screen.getByRole('heading', { name: 'Assistant · commentary' })).toBeInTheDocument()
    expect(screen.getByLabelText('Text item 7').textContent).toBe(exact)
    expect(screen.getByLabelText('Text item 7')).toHaveClass('history-prose')
    expect(container.querySelector('script')).toBeNull()
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

  it('keeps known text blocks readable and discloses unknown blocks in Raw', () => {
    const item = { type: 'message', role: 'user', content: [
      { type: 'input_text', text: exact }, { type: 'output_text', text: ' second\n' },
      { type: 'future_media', bytes: 'unknown-payload' },
    ] }
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Text item 7 block 1').textContent).toBe(exact)
    expect(screen.getByLabelText('Text item 7 block 2').textContent).toBe(' second\n')
    expect(screen.getByText(/Unknown content is retained/)).toBeInTheDocument()
    expect(screen.queryByLabelText('Raw item 7')).toBeNull()
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    fireEvent.click(screen.getByRole('button', { name: 'Hide Raw item 7' }))
    expect(screen.queryByLabelText('Raw item 7')).toBeNull()
  })

  it.each([
    [{type:'message',role:'assistant',content:[{type:'refusal',refusal:exact}]}, 'Text'],
    [{type:'reasoning',summary:[],content:[{type:'reasoning_text',text:exact}]}, 'Reasoning'],
    [{ type: 'reasoning', summary: [{ type: 'summary_text', text: exact }] }, 'Summary'],
    [{ type: 'function_call', name: 'f', call_id: 'call λ', arguments: 'arguments as exact text' }, 'Arguments'],
    [{ type: 'custom_tool_call', name: 'cell', call_id: 'call λ', input: exact }, 'Input'],
    [{ type: 'function_call_output', call_id: 'call λ', output: 'Tool failed: permission denied\n' }, 'Result'],
    [{ type: 'custom_tool_call_output', call_id: 'call λ', output: exact }, 'Result'],
    [{ type: 'configuration_update', reasoning: { effort: 'high' } }, 'Reasoning effort'],
  ] as const)('retains the original text for %j', (item, label) => {
    render(<HistoryItem entry={entry(item)} />)
    const source = 'content' in item || 'summary' in item ? exact : 'arguments' in item ? item.arguments : 'input' in item ? item.input
      : 'output' in item ? item.output : item.reasoning.effort
    if ('type' in item && item.type === 'reasoning') {
      const disclosure = screen.getByText(/show readable text/i)
      expect(disclosure).toBeInTheDocument()
      expect(disclosure.tagName).toBe('SUMMARY')
      expect(disclosure.closest('details')).not.toHaveAttribute('open')
      fireEvent.click(disclosure)
    }
    expect(screen.getByLabelText(`${label} item 7`).textContent).toBe(source)
    if ('type' in item && item.type.includes('call')) expect(screen.getByLabelText(`${label} item 7`)).toHaveClass('history-code')
    if ('type' in item && (item.type === 'function_call' || item.type === 'custom_tool_call')) {
      expect(screen.getByRole('heading')).toHaveTextContent('Model tool call')
      expect(screen.getByRole('heading')).not.toHaveTextContent('call λ')
    }
    if ('type' in item && (item.type === 'function_call_output' || item.type === 'custom_tool_call_output')) {
      expect(screen.getByRole('heading', { name: 'Tool response' })).toBeInTheDocument()
      expect(screen.getByRole('heading')).not.toHaveTextContent('call λ')
      expect(screen.getByLabelText(`${label} item 7`)).toBeVisible()
      fireEvent.click(screen.getByText('Message details'))
      fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
      expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    }
  })

  it.each([null, false, 0, { type: 'future_item', extra: ['λ'] }, { type: 'custom_tool_call_output', call_id: 'c', output: false }, { type: 'custom_tool_call', call_id: 'c', name: 'f', input: { z: 1 } }])('shows genuine arbitrary or unfamiliar JSON %j', (item) => {
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    expect(screen.queryByText(/success/i)).toBeNull()
  })

  it('renders valid object function arguments as structured JSON with exact call metadata and a Raw toggle', () => {
    const argumentsObject = { z: 1, nested: { unicode: '  λ\n🐈  ', nullable: null }, choices: [false, 0] }
    const item = { type: 'function_call', call_id: 'call λ', name: 'structured', arguments: argumentsObject }
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByRole('heading', { name: 'Model tool call · structured' })).toBeInTheDocument()
    expect(screen.getByRole('heading')).not.toHaveTextContent('call λ')
    expect(screen.getByLabelText('Arguments JSON item 7').textContent).toBe(JSON.stringify(argumentsObject, null, 2))
    expect(screen.getByLabelText('Arguments JSON item 7')).toHaveClass('history-code')
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('bounds readable and raw previews and expands only the fetched full text', () => {
    const text = 'λ'.repeat(2100) + '\n original tail'
    render(<HistoryItem entry={entry({ type: 'message', role: 'assistant', content: text })} />)
    expect(screen.getByLabelText('Text item 7').textContent).toBe(text.slice(0, 2000))
    fireEvent.click(screen.getByRole('button', { name: 'Expand Text item 7' }))
    expect(screen.getByLabelText('Text item 7').textContent).toBe(text)
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent?.length).toBe(2000)
    expect(screen.getByLabelText('Raw item 7')).toHaveClass('history-code')
    fireEvent.click(screen.getByRole('button', { name: 'Expand Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toContain('original tail')
  })

  it('does not format known raw JSON before its toggle is opened', () => {
    const item = { type: 'message', role: 'user', content: exact }
    const serialize = vi.spyOn(JSON, 'stringify')
    render(<HistoryItem entry={entry(item)} />)
    expect(serialize.mock.calls.some(([value]) => value === item)).toBe(false)
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(serialize.mock.calls.some(([value]) => value === item)).toBe(true)
    serialize.mockRestore()
  })

  it('never splits a Unicode surrogate pair at the collapsed preview boundary', () => {
    const text = 'x'.repeat(1999) + '🐈 λ'
    render(<HistoryItem entry={entry({ type: 'message', role: 'user', content: text })} />)
    expect(screen.getByLabelText('Text item 7').textContent).toBe('x'.repeat(1999))
    fireEvent.click(screen.getByRole('button', { name: 'Expand Text item 7' }))
    expect(screen.getByLabelText('Text item 7').textContent).toBe(text)
  })
})
