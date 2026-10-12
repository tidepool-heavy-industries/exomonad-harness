import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import RecoveryControls from './RecoveryControls'
import type { InferenceBlock } from './protocol'

const target = { run: 'run', actor: '/root', incarnation: 'one' }
const block: InferenceBlock = { request: 'head', generation: '3', reason: { kind: 'continuity_unproven' } }

describe('recovery decisions', () => {
  it.each(['exact', 'visible_history'] as const)('sends %s only after explicit selection and authorization', mode => {
    const submit = vi.fn().mockReturnValue({ kind: 'retained', operationId: 'operation', send: 'unknown' })
    const mounted = render(<RecoveryControls target={target} block={block} ready={false} submit={submit} onResult={vi.fn()} />)
    expect(submit).not.toHaveBeenCalled()
    mounted.rerender(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} />)
    expect(submit).not.toHaveBeenCalled()
    expect(screen.getByRole('button')).toBeDisabled()
    fireEvent.click(screen.getAllByRole('radio')[mode === 'exact' ? 0 : 1]!)
    fireEvent.click(screen.getByRole('button'))
    expect(submit).toHaveBeenCalledExactlyOnceWith({ action: 'recover', target, expected_block: block, mode })
    fireEvent.click(screen.getByRole('button'))
    expect(submit).toHaveBeenCalledOnce()
  })
  it('invalidates a decision when the head or block generation changes', () => {
    const submit = vi.fn()
    const mounted = render(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} />)
    fireEvent.click(screen.getAllByRole('radio')[0]!)
    expect(screen.getByRole('button')).toBeEnabled()
    mounted.rerender(<RecoveryControls target={target} block={{ ...block, request: 'new-head', generation: '4' }} ready submit={submit} onResult={vi.fn()} />)
    expect(screen.getByRole('button')).toBeDisabled()
    expect(screen.getAllByRole('radio').every(radio => !(radio as HTMLInputElement).checked)).toBe(true)
    expect(submit).not.toHaveBeenCalled()
  })
})

it('allows a new explicit decision after authoritative refusal without retrying automatically', () => {
  const id = '11111111-1111-4111-8111-111111111111'
  const submit = vi.fn().mockReturnValue({ kind: 'retained', operationId: id, send: 'sent' })
  const mounted = render(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} />)
  fireEvent.click(screen.getAllByRole('radio')[0]!)
  fireEvent.click(screen.getByRole('button'))
  mounted.rerender(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} operations={[{
    hostRun: target.run, authority: 'receipt', state: 'refused',
    submission: { operation_id: id, command: { action: 'recover', target, expected_block: block, mode: 'exact' } },
    receipt: { commandId: id, target, outcome: 'refused', reason: 'Continuity unavailable' },
  }]} />)
  expect(submit).toHaveBeenCalledOnce()
  fireEvent.click(screen.getAllByRole('radio')[1]!)
  fireEvent.click(screen.getByRole('button'))
  expect(submit).toHaveBeenLastCalledWith({ action: 'recover', target, expected_block: block, mode: 'visible_history' })
})
