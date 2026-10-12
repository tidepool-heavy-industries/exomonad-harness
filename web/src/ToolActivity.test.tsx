import { describe, expect, it } from 'vitest'
import { render, screen } from '@testing-library/react'
import ToolActivity, { resultExcerpt } from './ToolActivity'
import { PresentationMemory } from './presentation-state'

describe('tool result excerpts and evidence', () => {
  it('uses the first nonempty literal result line without claiming an outcome', () => {
    expect(resultExcerpt({ output: '\n  \n  Tool failed: permission denied\nnext line' })).toBe('Tool failed: permission denied')
    expect(resultExcerpt({ output: [{ type: 'image', data: 'opaque' }, { type: 'text', text: '\n' }, { type: 'output_text', text: 'retained text\nmore' }] })).toBe('retained text')
    expect(resultExcerpt({ output: [{ type: 'image', data: 'opaque' }] })).toBeUndefined()
    expect(resultExcerpt({ output: false })).toBeUndefined()
    expect(resultExcerpt({ output: ' '.repeat(10000) + 'x'.repeat(10000) })).toBe('x'.repeat(180) + '…')
    expect(resultExcerpt({ output: 'x'.repeat(179) + '🐈 tail' })).toBe('x'.repeat(179) + '…')
  })

  it('groups input, result publication, raw evidence and execution under one disclosure', () => {
    const entry = { kind: 'message' as const, sequence: '1', requestId: 'req', position: '0', hash: 'a'.repeat(64), providerItemId: null, inputOperation: null, tool: null,
      item: { type: 'custom_tool_call', name: 'haskell', call_id: 'c', input: 'observe value' } }
    const result = { ...entry, sequence: '2', position: '1', hash: 'b'.repeat(64), item: { type: 'custom_tool_call_output', call_id: 'c', output: 'retained value\nsecond line' } }
    const { container } = render(<PresentationMemory><ToolActivity row={{ kind: 'activity', key: 'tool', entry, results: [result] }} streamingActive={false} /></PresentationMemory>)
    expect(screen.getByText('Result excerpt:').parentElement).toHaveTextContent('retained value')
    expect(screen.getByLabelText('Result 2')).toBeVisible()
    expect(container.querySelectorAll('details')).toHaveLength(1)
    expect(screen.getByText('Execution log')).not.toBeVisible()
    expect(screen.getByText(/Sequence 2/)).not.toBeVisible()
    expect(screen.queryByLabelText('Raw tool item')).toBeNull()
  })
})
