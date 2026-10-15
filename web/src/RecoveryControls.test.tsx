import { fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import RecoveryControls from './RecoveryControls'
import type { InferenceBlock } from './protocol'
const target = { run: 'run', actor: '/root', incarnation: 'inc' }
const block: InferenceBlock = { request: 'head', generation: '3', reason: { kind: 'continuity_unproven' } }
const id = '11111111-1111-4111-8111-111111111111'
function retained() { return { kind: 'retained', operationId: id, send: 'sent' } as const }

describe('response continuation controls', () => {
  it('requires one explicit Retry and never submits on reconnect or repeated activation', () => {
    const submit = vi.fn(retained)
    const mounted = render(<RecoveryControls target={target} block={block} ready={false} submit={submit} onResult={vi.fn()} />)
    expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled()
    mounted.rerender(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} />)
    expect(submit).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(submit).toHaveBeenCalledExactlyOnceWith({ action: 'recover', target, expected_block: block, mode: 'exact' })
    fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
    expect(submit).toHaveBeenCalledOnce()
    expect(screen.queryByRole('radio')).toBeNull()
  })
  it('requires confirmation before visible continuation and invalidates it when the block changes', () => {
    const submit = vi.fn(retained)
    const mounted = render(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} continuationFailure="exact_projection_unavailable" />)
    fireEvent.click(screen.getByRole('button', { name: 'Review continuation' }))
    expect(screen.getByRole('group', { name: 'Continue without some reasoning context?' })).toHaveFocus()
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    expect(screen.getByRole('button', { name: 'Review continuation' })).toHaveFocus()
    expect(submit).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: 'Review continuation' }))
    const next = { ...block, generation: '4' }
    mounted.rerender(<RecoveryControls target={target} block={next} ready submit={submit} onResult={vi.fn()} continuationFailure="exact_projection_unavailable" />)
    expect(screen.queryByRole('button', { name: 'Continue without this context' })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: 'Review continuation' }))
    fireEvent.click(screen.getByRole('button', { name: 'Continue without this context' }))
    expect(submit).toHaveBeenCalledExactlyOnceWith({ action: 'recover', target, expected_block: next, mode: 'visible_history' })
  })
  it('never offers visible fallback without owner permission and keeps provider diagnostics in details', () => {
    const diagnostic = { code: 'subscription_sharing_usage_limit_exceeded', message: 'Provider quota response' }
    const quota: InferenceBlock = { ...block, reason: { kind: 'provider_stream_failure', event: 'response_failed', diagnostic } }
    render(<RecoveryControls target={target} block={quota} ready submit={vi.fn()} onResult={vi.fn()} />)
    expect(screen.queryByRole('button', { name: 'Review continuation' })).toBeNull()
    expect(screen.getByText(/Select another saved model account/)).toBeVisible()
    fireEvent.click(screen.getByText('Details'))
    expect(screen.getByText(diagnostic.message)).toBeVisible()
  })
})

it('cannot carry a context-loss confirmation across actor incarnation changes', () => {
  const submit = vi.fn(retained)
  const mounted = render(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} continuationFailure="exact_projection_unavailable" />)
  fireEvent.click(screen.getByRole('button', { name: 'Review continuation' }))
  mounted.rerender(<RecoveryControls target={{ ...target, incarnation: 'new' }} block={block} ready submit={submit} onResult={vi.fn()} continuationFailure="exact_projection_unavailable" />)
  expect(screen.queryByRole('button', { name: 'Continue without this context' })).toBeNull()
  expect(submit).not.toHaveBeenCalled()
})

it.each(['uncertain_effects', 'continuity_unproven', 'stale'] as const)('never offers visible continuation for %s', continuationFailure => {
  const submit = vi.fn(retained)
  render(<RecoveryControls target={target} block={block} ready submit={submit} onResult={vi.fn()} continuationFailure={continuationFailure} />)
  expect(screen.queryByRole('button', { name: 'Review continuation' })).toBeNull()
  expect(screen.queryByRole('button', { name: 'Continue without this context' })).toBeNull()
  if (continuationFailure === 'uncertain_effects') expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled()
  expect(submit).not.toHaveBeenCalled()
})

it('reuses an unresolved exact operation after reload and never routes ordinary Retry into a visible operation', () => {
  const submit = vi.fn(retained)
  const retry = vi.fn(retained)
  const record = { hostRun: target.run, authority: 'local' as const, state: 'queued' as const,
    submission: { operation_id: id, command: { action: 'recover' as const, target, expected_block: block, mode: 'exact' as const } } }
  const mounted = render(<RecoveryControls target={target} block={block} ready submit={submit} retry={retry} onResult={vi.fn()} operations={[record]} />)
  fireEvent.click(screen.getByRole('button', { name: 'Retry' }))
  expect(retry).toHaveBeenCalledExactlyOnceWith(record.submission)
  expect(submit).not.toHaveBeenCalled()
  const visible = { ...record, submission: { ...record.submission, command: { ...record.submission.command, mode: 'visible_history' as const } } }
  mounted.unmount()
  render(<RecoveryControls target={target} block={block} ready submit={submit} retry={retry} onResult={vi.fn()} operations={[visible]} continuationFailure="exact_projection_unavailable" />)
  expect(screen.getByRole('button', { name: 'Retry' })).toBeDisabled()
  expect(retry).toHaveBeenCalledOnce()
  fireEvent.click(screen.getByRole('button', { name: 'Review continuation' }))
  expect(retry).toHaveBeenCalledOnce()
  fireEvent.click(screen.getByRole('button', { name: 'Continue without this context' }))
  expect(retry).toHaveBeenLastCalledWith(visible.submission)
  expect(submit).not.toHaveBeenCalled()
})
