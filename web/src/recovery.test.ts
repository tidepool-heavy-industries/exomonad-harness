import { describe, expect, it } from 'vitest'
import { toViewModel } from './integration'
import { normalizeSnapshot } from './protocol'
import { commandIsCurrent, currentRecoveryBlock, viewCommandIsCurrent } from './recovery'
import { isHostCommand, isHostInferenceState, isCommandReceipt, isEmbeddedCommandRecord, isSnapshot, sameHostCommand, type HostActorProjection, type InferenceBlock } from './protocol'

const target = { run: 'run', actor: '/root', incarnation: 'one' }
const block = { request: 'head', generation: '3', reason: { kind: 'continuity_unproven' } } satisfies InferenceBlock
const actor: HostActorProjection = { identity: target, kind: 'model', lifecycle: 'waiting', parent: null,
  modelConversation: 'conversation', modelHeadRequest: 'head', inference: { state: 'blocked', block } }
const command = { action: 'recover' as const, target, expected_block: block, mode: 'exact' as const }

describe('explicit recovery authority', () => {
  it('binds both recovery modes to exact identity, head, and block generation independently of lifecycle', () => {
    expect(currentRecoveryBlock(actor)).toEqual(block)
    for (const mode of ['exact', 'visible_history'] as const) {
      expect(commandIsCurrent({ ...command, mode }, actor)).toBe(true)
      expect(commandIsCurrent({ ...command, mode }, { ...actor, modelHeadRequest: 'other' })).toBe(false)
      expect(commandIsCurrent({ ...command, mode }, { ...actor, inference: { state: 'blocked', block: { ...block, generation: '4' } } })).toBe(false)
    }
    for (const lifecycle of ['retiring', 'retired', 'lost'] as const) {
      expect(commandIsCurrent(command, { ...actor, lifecycle })).toBe(false)
      expect(currentRecoveryBlock({ ...actor, lifecycle })).toBeUndefined()
    }
    expect(commandIsCurrent({ action: 'input', target, text: 'queued' }, actor)).toBe(true)
    expect(commandIsCurrent({ action: 'retire', target }, actor)).toBe(true)
    const data = toViewModel(normalizeSnapshot({ seq: '1', hostRun: target.run, actors: [actor], conversations: [], requests: [], jobs: [], envelopes: [] }))
    expect(viewCommandIsCurrent(command, data)).toBe(true)
    expect(viewCommandIsCurrent({ ...command, target: { ...target, incarnation: 'two' } }, data)).toBe(false)
    expect(viewCommandIsCurrent(command, { ...data, hostRun: 'another' })).toBe(false)
  })
  it('permits only the original operation while recovering and no recovery while idle or running', () => {
    const operationId = '11111111-1111-4111-8111-111111111111'
    const recovering: HostActorProjection = { ...actor, inference: { state: 'recovering', operationId, block } }
    expect(commandIsCurrent(command, recovering)).toBe(false)
    expect(commandIsCurrent(command, recovering, operationId)).toBe(true)
    expect(commandIsCurrent(command, { ...recovering, modelHeadRequest: 'visible-successor' }, operationId)).toBe(true)
    expect(commandIsCurrent(command, recovering, '22222222-2222-4222-8222-222222222222')).toBe(false)
    for (const state of ['idle', 'running'] as const) expect(commandIsCurrent(command, { ...actor, inference: { state } })).toBe(false)
  })
  it('validates typed inference and compares recovery commands without relying on JSON field ordering', () => {
    expect(isHostCommand(command)).toBe(true)
    expect(isHostCommand({ ...command, mode: 'automatic' })).toBe(false)
    expect(isHostCommand({ ...command, expected_block: { ...block, generation: Number.MAX_SAFE_INTEGER + 1 } })).toBe(false)
    expect(sameHostCommand(command, { ...command, expected_block: { reason: block.reason, generation: block.generation, request: block.request } })).toBe(true)
    expect(sameHostCommand(command, { ...command, mode: 'visible_history' })).toBe(false)
    expect(sameHostCommand(command, { ...command, expected_block: { ...block, generation: '4' } })).toBe(false)
    expect(isSnapshot({ seq: '1', actors: [actor], conversations: [], requests: [], jobs: [], envelopes: [] })).toBe(true)
    for (const reason of [
      { kind: 'stream_interrupted', cause: 'missing_completion' }, { kind: 'stream_interrupted', cause: 'read_failed' },
      { kind: 'provider_stream_failure', event: 'response_failed' }, { kind: 'provider_stream_failure', event: 'error' },
      { kind: 'request_rejected', failure: { kind: 'authentication' } }, { kind: 'host_interrupted' },
    ]) expect(isHostInferenceState({ state: 'blocked', block: { ...block, reason } })).toBe(true)
    expect(isHostInferenceState({ state: 'recovering', block, operationId: 'not-an-operation' })).toBe(false)
    expect(isHostInferenceState({ state: 'blocked', block: { ...block, reason: { kind: 'provider_stream_failure', event: 'arbitrary' } } })).toBe(false)
  })
})


it('validates recovery receipts and exact journal correlation, including required omission evidence', () => {
  const operationId = '11111111-1111-4111-8111-111111111111'
  const receipt = { commandId: operationId, target, outcome: 'recovery_authorized', block, mode: 'exact',
    projection: { omitted: [], unknownResponseCompleteness: ['head'] } }
  const record = { operationId, command, state: 'recovery_authorized', receipt, envelopeId: null }
  expect(isCommandReceipt(receipt)).toBe(true)
  expect(isEmbeddedCommandRecord(record)).toBe(true)
  expect(isCommandReceipt({ ...receipt, projection: { omitted: [] } })).toBe(false)
  expect(isCommandReceipt({ ...receipt, projection: { omitted: [42], unknownResponseCompleteness: [] } })).toBe(false)
  expect(isEmbeddedCommandRecord({ ...record, receipt: { ...receipt, mode: 'visible_history' } })).toBe(false)
  expect(isEmbeddedCommandRecord({ ...record, receipt: { ...receipt, block: { ...block, generation: '4' } } })).toBe(false)
  expect(isEmbeddedCommandRecord({ ...record, command: { action: 'input', target, text: 'queued' } })).toBe(false)
})
