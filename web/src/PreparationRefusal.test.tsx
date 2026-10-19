import { render, screen, fireEvent } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import RecoveryControls from './RecoveryControls'
import { isInferenceBlock, sameInferenceBlock, type InferenceBlock } from './protocol'

const block: InferenceBlock = { request: 'head', generation: '7', reason: { kind: 'request_preparation_refused', route: 'deep_seek', semantics: 'message' } }

describe('local preparation refusal', () => {
  it('validates the generated schema and compares exact typed refusal facts', () => {
    expect(isInferenceBlock(block)).toBe(true)
    expect(sameInferenceBlock(block, structuredClone(block))).toBe(true)
    for (const changed of [
      { ...block, generation: '8' },
      { ...block, reason: { ...block.reason, route: 'chat_gpt_plan' } },
      { ...block, reason: { ...block.reason, semantics: 'tool_declaration' } },
    ] as InferenceBlock[]) expect(sameInferenceBlock(block, changed)).toBe(false)
    expect(isInferenceBlock({ ...block, reason: { ...block.reason, semantics: 'unknown' } })).toBe(false)
    expect(isInferenceBlock({ ...block, reason: { kind: 'request_preparation_refused', route: 'deep_seek' } })).toBe(false)
  })
  it('explains the local refusal and submits only explicit exact recovery', () => {
    const submit = vi.fn(() => ({ kind: 'retained', operationId: '11111111-1111-4111-8111-111111111111', send: 'sent' } as const))
    const target = { run: 'run', actor: '/root', incarnation: 'first' }
    render(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} />)
    expect(screen.getByRole('region', { name: 'Request preparation blocked' })).toBeInTheDocument()
    expect(screen.getByText(/No provider request was sent/)).toBeInTheDocument()
    expect(screen.getByText(/unsupported semantics: message/)).toBeInTheDocument()
    expect(submit).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Retry after correction' }))
    expect(submit).toHaveBeenCalledExactlyOnceWith({ action: 'recover', target, expected_block: block, mode: 'exact' })
  })
})
