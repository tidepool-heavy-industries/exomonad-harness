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
  it('shows a readable role and exact escaped string content without executing HTML', () => {
    const { container } = render(<HistoryItem entry={entry({ type: 'message', role: 'assistant', phase: 'commentary', content: exact })} />)
    expect(screen.getByRole('heading', { name: 'Assistant' })).toBeInTheDocument()
    expect(screen.getByLabelText('Text item 7').textContent).toBe(exact)
    expect(container.querySelector('script')).toBeNull()
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
    [{ type: 'function_call', name: 'f', call_id: 'call λ', arguments: ' { "z":1, "a" : 2 }\n' }, 'Arguments'],
    [{ type: 'custom_tool_call', name: 'cell', call_id: 'call λ', input: exact }, 'Input'],
    [{ type: 'function_call_output', call_id: 'call λ', output: ' { "z":1, "a" : 2 }\n' }, 'Output'],
    [{ type: 'custom_tool_call_output', call_id: 'call λ', output: exact }, 'Output'],
    [{ type: 'configuration_update', reasoning: { effort: 'high' } }, 'Reasoning effort'],
  ] as const)('retains the original text for %j', (item, label) => {
    render(<HistoryItem entry={entry(item)} />)
    const source = 'content' in item || 'summary' in item ? exact : 'arguments' in item ? item.arguments : 'input' in item ? item.input
      : 'output' in item ? item.output : item.reasoning.effort
    expect(screen.getByLabelText(`${label} item 7`).textContent).toBe(source)
    if ('type' in item && item.type.includes('call')) expect(screen.getByRole('heading').textContent).toContain(item.type)
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
    expect(screen.getByRole('heading', { name: 'function_call · structured · call call λ' })).toBeInTheDocument()
    expect(screen.getByLabelText('Arguments JSON item 7').textContent).toBe(JSON.stringify(argumentsObject, null, 2))
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
