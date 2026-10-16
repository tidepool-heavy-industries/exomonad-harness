import { describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import HistoryItem from './HistoryItem'

const entry = (item: unknown) => ({ position: '7', hash: 'a'.repeat(64), byteLen: '123', item })
const exact = '  λ 🐈\n"quoted" \\  <script>alert(1)</script>  '

const haskellCode = '-- λ\nlet prompt = "literal \\n"\n  observe prompt\n'
it.each([
  { fields: { type: 'custom_tool_call', input: haskellCode }, source: haskellCode },
  { fields: { type: 'custom_tool_call', input: '["a","b"]' }, source: '["a","b"]' },
  { fields: { type: 'function_call', arguments: JSON.stringify({ code: haskellCode }) }, source: haskellCode },
  { fields: { type: 'function_call', arguments: { code: haskellCode } }, source: haskellCode },
  { fields: { type: 'function_call', arguments: JSON.stringify(haskellCode) }, source: haskellCode },
])('presents the actual Haskell source and copies it exactly for $fields.type', async ({ fields, source }) => {
  const item = { ...fields, name: 'haskell', call_id: 'cell' }
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
  const writeText = vi.fn().mockResolvedValue(undefined)
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  try {
    const { container } = render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Input item 7').textContent).toBe(source)
    expect(container.querySelector('code.language-haskell')).not.toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Copy Input item 7' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledExactlyOnceWith(source))
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  } finally {
    if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor)
    else Reflect.deleteProperty(navigator, 'clipboard')
  }
})

it('keeps unrelated code arguments and malformed Haskell envelopes visible without guessing a language', () => {
  const { container, rerender } = render(<HistoryItem entry={entry({ type: 'function_call', name: 'shell', call_id: 'call', arguments: JSON.stringify({ code: 'let value = 42\nobserve value' }) })} />)
  expect(container.querySelector('code.language-haskell')).toBeNull()
  expect(screen.getByLabelText('Arguments item 7').textContent).toContain('"code"')
  rerender(<HistoryItem entry={entry({ type: 'function_call', name: 'haskell', call_id: 'call', arguments: '{"code":' })} />)
  expect(container.querySelector('code.language-haskell')).toBeNull()
  expect(screen.getByLabelText('Arguments item 7').textContent).toBe('{"code":')
})

function showRaw() {
  fireEvent.click(screen.getByText('Message details'))
  fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
}

describe('retained Item presentation', () => {
  it('omits opaque reasoning with absent, empty, or whitespace-only summaries', () => {
    const item = { type: 'reasoning', encrypted_content: 'opaque' }
    const { container, rerender } = render(<HistoryItem entry={entry(item)} />)
    expect(container.textContent).toBe('')
    rerender(<HistoryItem entry={entry({ ...item, summary: [] })} />)
    expect(container.textContent).toBe('')
    rerender(<HistoryItem entry={entry({ ...item, summary: [{ type: 'summary_text', text: '   \n' }] })} />)
    expect(container.textContent).toBe('')
  })

  it.each([
    { type: 'reasoning', summary: [{ type: 'future_summary', payload: 'λ 🐈' }], encrypted_content: 'opaque' },
    { type: 'reasoning', content: { future: 'content shape' } },
  ])('preserves unfamiliar reasoning blocks and shapes for exact raw inspection: %j', item => {
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByText(/Unknown content is retained/)).toBeVisible()
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('renders assistant Markdown and preserves the entire original wire item for inspection', () => {
    const item = { type: 'message', role: 'assistant', phase: 'commentary', content: '# Result\n\n**Ready** with `code`.\n\n<script>alert(1)</script>' }
    const { container } = render(<HistoryItem entry={entry(item)} listItem={false} />)
    expect(screen.queryByRole('listitem')).toBeNull()
    expect(screen.getByRole('heading', { name: 'Assistant · commentary' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Result' })).toBeInTheDocument()
    expect(container.querySelector('strong')?.textContent).toBe('Ready')
    expect(container.querySelector('script')).toBeNull()
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  })

  it('keeps human text exact and discloses unknown blocks in Raw', () => {
    const text = `${exact}${'x'.repeat(1999)}🐈 λ\n${'α'.repeat(5000)}\n original tail`
    const item = { type: 'message', role: 'user', content: [
      { type: 'input_text', text }, { type: 'output_text', text: ' second\n' },
      { type: 'future_media', bytes: 'unknown-payload' },
    ] }
    const { container } = render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Text item 7 block 1').textContent).toBe(text)
    expect(screen.getByLabelText('Text item 7 block 2').textContent).toBe(' second\n')
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

  it.each([null, { type: 'future_item', extra: [null, false, 0, 'λ'] }, { type: 'custom_tool_call_output', call_id: 'c', output: false }, { type: 'custom_tool_call', call_id: 'c', name: 'f', input: { z: 1 } }])('shows arbitrary or unfamiliar JSON %j', item => {
    render(<HistoryItem entry={entry(item)} />)
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
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
    expect(screen.getByText(/a{64}/)).not.toBeVisible()
    expect(serialize.mock.calls.some(([value]) => value === item)).toBe(false)
    showRaw()
    expect(screen.getByText(/a{64}/)).toBeVisible()
    expect(serialize.mock.calls.some(([value]) => value === item)).toBe(true)
    serialize.mockRestore()
  })

  it.each(['function_call', 'custom_tool_call', 'function_call_output', 'custom_tool_call_output'])('copies exact retained source for formatted %s', async type => {
    const source = type === 'custom_tool_call_output' ? ' [ { "z": 1, "cat" : "λ 🐈" } ]\n' : ' { "z": 1, "cat" : "λ 🐈" }\n'
    const item = type === 'function_call' ? { type, id: 'retained-item', name: 'tool', call_id: 'exact-call', arguments: source }
      : type === 'custom_tool_call' ? { type, id: 'retained-item', name: 'cell', call_id: 'exact-call', input: source }
      : { type, id: 'retained-item', call_id: 'exact-call', output: source }
    const label = type === 'function_call' ? 'Arguments' : type === 'custom_tool_call' ? 'Input' : 'Result'
    const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
    const writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
    try {
      render(<HistoryItem entry={entry(item)} />)
      const metadata = screen.getByText('Message details').closest('details')!
      expect(metadata).not.toHaveAttribute('open')
      expect(screen.getByText(/Call ID:/)).not.toBeVisible()
      expect(screen.getByRole('heading')).not.toHaveTextContent('exact-call')
      if (label === 'Result') {
        expect(screen.getByLabelText(`${label} item 7`)).not.toBeVisible()
        fireEvent.click(screen.getByText(/^JSON result/))
      }
      expect(screen.getByLabelText(`${label} item 7`).textContent).toBe(JSON.stringify(JSON.parse(source), null, 2))
      fireEvent.click(screen.getByRole('button', { name: `Copy ${label} item 7` }))
      await waitFor(() => expect(writeText).toHaveBeenCalledWith(source))
      showRaw()
      expect(metadata).toHaveTextContent('retained-item')
      expect(metadata).toHaveTextContent('exact-call')
      expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
    } finally {
      if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor)
      else Reflect.deleteProperty(navigator, 'clipboard')
    }
  })

  it('bounds long JSON previews while keeping nested response data available on expansion', () => {
    const key = 'a'.repeat(60), value = 'b'.repeat(200)
    const response = { [key]: value, ready_results: [{ origin: { actor: '/root' } }] }
    const output = JSON.stringify(response)
    render(<HistoryItem entry={entry({ type: 'function_call_output', call_id: 'call-1', output })} />)
    const disclosure = screen.getByText(/^JSON result/)
    expect(disclosure).not.toHaveTextContent(key)
    expect(disclosure).not.toHaveTextContent(value)
    expect(screen.getByLabelText('Result item 7')).not.toBeVisible()
    fireEvent.click(disclosure)
    expect(screen.getByLabelText('Result item 7').textContent).toBe(JSON.stringify(response, null, 2))
  })

  it('labels final assistant text and exposes unknown phases through Raw', () => {
    const finalItem = { type: 'message', role: 'assistant', phase: 'final', content: 'Done.' }
    const { rerender } = render(<HistoryItem entry={entry(finalItem)} />)
    expect(screen.getByRole('heading', { name: 'Assistant · final' })).toBeInTheDocument()
    rerender(<HistoryItem entry={entry({ ...finalItem, phase: 'final_answer' })} />)
    expect(screen.getByRole('heading', { name: 'Assistant · final' })).toBeInTheDocument()
    expect(screen.queryByText(/Unknown content is retained/)).toBeNull()
    const futureItem = { type: 'message', role: 'assistant', phase: 'future_phase', content: 'Retained.' }
    rerender(<HistoryItem entry={entry(futureItem)} />)
    expect(screen.getByRole('heading', { name: 'Assistant · unknown phase (future_phase)' })).toBeInTheDocument()
    expect(screen.getByText(/Unknown content is retained/)).toBeInTheDocument()
    fireEvent.click(screen.getByText('Message details'))
    fireEvent.click(screen.getByRole('button', { name: 'Show Raw item 7' }))
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(futureItem, null, 2))
  })

})

it('uses the Store-certified model item for ordinary output and copies while preserving canonical evidence', async () => {
  const output = JSON.stringify({ result: { privateEvidence: 'canonical native payload' }, presentation: 'not a browser policy' })
  const item = { type: 'custom_tool_call_output', call_id: 'native', output }
  const modelItem = { ...item, output: 'Authored result only' }
  const writeText = vi.fn().mockResolvedValue(undefined)
  const descriptor = Object.getOwnPropertyDescriptor(navigator, 'clipboard')
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } })
  try {
    render(<HistoryItem entry={{ ...entry(item), modelItem }} />)
    expect(screen.getByLabelText('Result item 7')).toHaveTextContent('Authored result only')
    expect(screen.queryByText(/canonical native payload/)).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Copy Result item 7' }))
    await waitFor(() => expect(writeText).toHaveBeenCalledExactlyOnceWith('Authored result only'))
    showRaw()
    expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(item, null, 2))
  } finally {
    if (descriptor) Object.defineProperty(navigator, 'clipboard', descriptor)
    else Reflect.deleteProperty(navigator, 'clipboard')
  }
})

it('does not infer an authored presentation from uncertified canonical JSON', () => {
  const output = JSON.stringify({ result: 42, presentation: 'do not extract this field' })
  render(<HistoryItem entry={entry({ type: 'custom_tool_call_output', call_id: 'native', output })} />)
  expect(screen.getByLabelText('Result item 7').textContent).toBe(JSON.stringify(JSON.parse(output), null, 2))
})

it('keeps unfamiliar certified items inspectable without falling back to canonical output', () => {
  const item = { type: 'custom_tool_call_output', call_id: 'native', output: 'canonical secret' }
  const modelItem = { type: 'future_item', authored: 'projected value' }
  render(<HistoryItem entry={{ ...entry(item), modelItem }} />)
  expect(screen.getByLabelText('Raw item 7').textContent).toBe(JSON.stringify(modelItem, null, 2))
  expect(screen.queryByText(/canonical secret/)).toBeNull()
  showRaw()
  expect(screen.getAllByLabelText('Raw item 7')[0]?.textContent).toBe(JSON.stringify(item, null, 2))
})
