import { fireEvent, render, screen } from '@testing-library/react'
import { afterEach, expect, it, vi } from 'vitest'
import { ActorComposer, OutgoingInputs } from './ActorControls'
import { clearDrafts } from './drafts'
import { continuationRefusal } from './recovery'
import { inputStatus } from './command-presentation'
import { applyCommandReceipt, retainCommand } from './pending-commands'
import { isEmbeddedCommandRecord, sameInferenceBlock, type HostCommand, type HostInferenceState, type InferenceBlock } from './protocol'
import type { HarnessViewModel } from './view-model'
import type { RouteState } from './client-contract'
const target = { run: 'run', actor: '/worker', incarnation: 'original' }
const route: RouteState = { screen: 'chat', selection: { kind: 'actor', identity: target }, global: false, messageFilters: {} }
const block = { request: 'head', generation: '3', reason: { kind: 'continuity_unproven' } } satisfies InferenceBlock
const id = '11111111-1111-4111-8111-111111111111'
const text = ' exact queued λ\nmessage '
function data(inference: HostInferenceState): HarnessViewModel { return { hostRun: 'run', actors: [{ id: JSON.stringify(['run', '/worker', 'original']), name: target.actor, run: target.run,
  incarnation: target.incarnation, kind: 'model', lifecycle: 'waiting', modelConversation: 'conversation', modelHeadRequest: 'head', inference }], nodes: [], timeline: [], inbox: [] } }
afterEach(clearDrafts)

it('sends exactly one input while blocked, preserves it as queued after continuation refusal, and never resubmits it for visible continuation', () => {
  const submit = vi.fn((_command: HostCommand) => ({ kind: 'retained', operationId: id, send: 'sent' } as const))
  const props = { route, unavailable: false, ready: true, onHostCommand: submit, operations: [] }
  const mounted = render(<><ActorComposer {...props} data={data({ state: 'blocked', block })} /><OutgoingInputs records={[]} /></>)
  fireEvent.change(screen.getByRole('textbox'), { target: { value: text } })
  fireEvent.keyDown(screen.getByRole('textbox'), { key: 'Enter', ctrlKey: true })
  expect(submit).toHaveBeenCalledExactlyOnceWith({ action: 'input', target, text })
  const receipt = { commandId: id, outcome: 'admitted' as const, target, envelopeId: '7', continuation: { state: 'refused' as const, block, reason: 'exact_projection_unavailable' as const } }
  const records = applyCommandReceipt(retainCommand([], target.run, { operation_id: id, command: { action: 'input', target, text } }), receipt)
  mounted.rerender(<><ActorComposer {...props} operations={records} data={data({ state: 'blocked', block })} /><OutgoingInputs records={records} /></>)
  expect(screen.getByLabelText('outgoing message').textContent).toBe(text)
  expect(screen.getByRole('textbox')).toHaveValue('')
  expect(records[0]!.state).toBe('input_admitted')
  expect(screen.queryByText('Sent to the host. Admission is shown in the message status.')).toBeNull()
  fireEvent.click(screen.getByRole('button', { name: 'Review continuation' }))
  expect(submit).toHaveBeenCalledOnce()
  fireEvent.click(screen.getByRole('button', { name: 'Continue without this context' }))
  expect(submit).toHaveBeenLastCalledWith({ action: 'recover', target, expected_block: block, mode: 'visible_history' })
  expect(submit.mock.calls.filter(call => call[0]?.action === 'input')).toHaveLength(1)
  expect(screen.getByLabelText('outgoing message').textContent).toBe(text)
})

it('renders host retry progress without starting timers or commands and preserves a draft across interrupted states', () => {
  const submit = vi.fn()
  const props = { route, unavailable: false, ready: true, onHostCommand: submit, operations: [] }
  const mounted = render(<ActorComposer {...props} data={data({ state: 'auto_retry', block, attempt: 1, maximum: 2, delayMs: 1000 })} />)
  const input = screen.getByRole('textbox')
  fireEvent.change(input, { target: { value: text } })
  expect(screen.queryByRole('button', { name: 'Retry' })).toBeNull()
  mounted.rerender(<ActorComposer {...props} data={data({ state: 'blocked', block })} />)
  expect(screen.getByRole('textbox')).toBe(input)
  expect(input).toHaveValue(text)
  expect(screen.getByRole('button', { name: 'Retry' })).toBeEnabled()
  expect(submit).not.toHaveBeenCalled()
})

it('binds fallback to exact owner evidence and refuses uncertain effects, stale blocks and other identities', () => {
  const uncertain = { ...block, continuationRefusal: 'uncertain_effects' as const }
  expect(continuationRefusal(target, uncertain, [])).toBe('uncertain_effects')
  expect(continuationRefusal(target, { ...block, continuationRefusal: 'exact_projection_unavailable' }, [])).toBe('exact_projection_unavailable')
  expect(sameInferenceBlock(block, uncertain)).toBe(false)
  const receipt = { commandId: id, outcome: 'continuation_refused' as const, target, block, reason: 'exact_projection_unavailable' as const }
  const records = applyCommandReceipt(retainCommand([], target.run, { operation_id: id, command: { action: 'recover', target, expected_block: block, mode: 'exact' } }), receipt)
  expect(records[0]!.state).toBe('refused')
  expect(continuationRefusal(target, { ...block, generation: '4' }, records)).toBeUndefined()
  expect(continuationRefusal({ ...target, incarnation: 'other' }, block, records)).toBeUndefined()
  expect(isEmbeddedCommandRecord({ operationId: id, command: records[0]!.submission.command, state: 'refused', envelopeId: null, receipt: { ...receipt, block: { ...block, generation: '4' } } })).toBe(false)
  const props = { route, unavailable: false, ready: true, onHostCommand: vi.fn(), operations: records }
  render(<ActorComposer {...props} data={data({ state: 'blocked', block: uncertain })} />)
  expect(screen.queryByRole('button', { name: 'Review continuation' })).toBeNull()
})

it('retains original message admission and flags contradictory continuation receipts', () => {
  const initial = retainCommand([], target.run, { operation_id: id, command: { action: 'input', target, text } })
  const receipt = { commandId: id, outcome: 'admitted' as const, target, envelopeId: '7', continuation: { state: 'queued' as const } }
  const admitted = applyCommandReceipt(initial, receipt)
  const contradictory = applyCommandReceipt(admitted, { ...receipt, continuation: { state: 'granted', block } })
  expect(contradictory[0]!.state).toBe('input_admitted')
  expect(contradictory[0]!.receipt).toEqual(receipt)
  expect(contradictory[0]!.issues).toContain('Contradictory continuation evidence.')
})

it('shows later request inclusion ahead of the immutable historical continuation refusal', () => {
  const initial = retainCommand([], target.run, { operation_id: id, command: { action: 'input', target, text } })
  const admitted = applyCommandReceipt(initial, { commandId: id, outcome: 'admitted', target, envelopeId: '7', continuation: { state: 'refused', block, reason: 'exact_projection_unavailable' } })
  const included = { ...admitted[0]!, inputObservation: { state: 'included' as const, request: 'continued' } }
  expect(inputStatus(included)).toBe('Included in a model request · execution outcome unobserved')
  expect(included.receipt?.outcome === 'admitted' && included.receipt.continuation?.state).toBe('refused')
})
