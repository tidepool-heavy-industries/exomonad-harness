import { fireEvent, render, screen } from '@testing-library/react'
import { expect, it } from 'vitest'
import OversizedOutput from './OversizedOutput'

it('shows the certified compact view and exact inspection reference without claiming canonical evidence was loaded', () => {
  render(<OversizedOutput entry={{ position: '2', hash: 'a'.repeat(64), byteLen: '500000',
    modelItem: { type: 'function_call_output', call_id: 'cell', output: 'Authored summary' },
    outputOperation: { origin: { kind: 'standalone', store: 'store', actor: '/root' }, request: 'request', call: 'cell' },
  }} />)
  expect(screen.getByLabelText('Result item 2')).toHaveTextContent('Authored summary')
  expect(screen.getByRole('status')).toHaveTextContent('500000 bytes')
  expect(screen.queryByRole('button', { name: /raw/i })).toBeNull()
  fireEvent.click(screen.getByText('Retained result reference'))
  expect(screen.getByText('Use inspectResult with this exact operation reference.')).toBeVisible()
  expect(screen.getByText('request', { exact: true })).toBeVisible()
  expect(screen.getByText('cell', { exact: true })).toBeVisible()
})

it('keeps the canonical oversize notice when no bounded certified presentation is available', () => {
  render(<OversizedOutput entry={{ position: '3', hash: 'b'.repeat(64), byteLen: '600000' }} />)
  expect(screen.getByRole('status')).toHaveTextContent('Full evidence remains in Store.')
  expect(screen.queryByLabelText('Result item 3')).toBeNull()
  expect(screen.queryByText(/Use inspectResult/)).toBeNull()
})

it('does not invent a raw canonical fallback for an unfamiliar certified shape', () => {
  render(<OversizedOutput entry={{ position: '4', hash: 'c'.repeat(64), byteLen: '700000', modelItem: { type: 'future', output: 'opaque' } }} />)
  expect(screen.getByText('The authored presentation cannot be displayed by this view.')).toBeVisible()
  expect(screen.queryByText('opaque')).toBeNull()
})
