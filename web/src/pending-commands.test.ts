import { beforeEach, describe, expect, it, vi } from 'vitest'
import { allocateOperationId, applyCommandStatus, readPendingCommands, retainCommand, writePendingCommands } from './pending-commands'

const command = { action: 'input' as const, target: { run: 'run-1', actor: '/root', incarnation: 'inc-1' }, text: 'continue' }

describe('retained embedded browser operations', () => {
  beforeEach(() => sessionStorage.clear())

  it('uses a browser-compatible UUID and retains the exact operation for explicit retry', () => {
    const operationId = allocateOperationId()
    expect(operationId).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    const submission = { operation_id: operationId, command }
    const retained = retainCommand([], 'run-1', submission)
    writePendingCommands(retained)
    expect(readPendingCommands()).toEqual(retained)
    expect(readPendingCommands()[0]?.submission).toEqual(submission)
  })

  it('reconciles status without changing the original command or operation identity', () => {
    const submission = { operation_id: '11111111-1111-4111-8111-111111111111', command }
    const retained = retainCommand([], 'run-1', submission)
    const next = applyCommandStatus(retained, 'run-1', {
      operationId: submission.operation_id,
      command,
      state: 'input_admitted',
      envelopeId: 42,
      receipt: null,
    })
    expect(next).toEqual([{ ...retained[0]!, state: 'input_admitted' }])
    expect(next[0]?.submission).toEqual(submission)
  })
})

it('keeps the same UUID isolated across runs and refuses edited contents in one run', () => {
  const operation_id = '11111111-1111-4111-8111-111111111111'
  const first = { operation_id, command }
  const second = { operation_id, command: { ...command, target: { ...command.target, run: 'run-2', incarnation: 'inc-2' } } }
  const records = retainCommand(retainCommand([], 'run-1', first), 'run-2', second)
  expect(records).toHaveLength(2)
  expect(() => retainCommand(records, 'run-1', { ...first, command: { ...command, text: 'edited' } })).toThrow(/different contents/)
  expect(() => retainCommand(records, 'run-1', second)).toThrow(/current exact host run/)
  writePendingCommands(records)
  expect(readPendingCommands()).toEqual(records)
})

it('rejects malformed retained identities, state, and mismatched host run', () => {
  const valid = retainCommand([], 'run-1', { operation_id: '11111111-1111-4111-8111-111111111111', command })[0]!
  sessionStorage.setItem('harness.embeddedCommands.v1', JSON.stringify([
    valid, { ...valid, hostRun: 'other' }, { ...valid, state: 'made_up' },
    { ...valid, submission: { ...valid.submission, operation_id: 'invalid' } },
    { ...valid, submission: { ...valid.submission, command: { action: 'interrupt', target: command.target, expected_round: 'invalid' } } },
  ]))
  expect(readPendingCommands()).toEqual([valid])
})

it('does not apply status for another incarnation or changed payload', () => {
  const operationId = '11111111-1111-4111-8111-111111111111'
  const records = retainCommand([], 'run-1', { operation_id: operationId, command })
  for (const altered of [
    { ...command, text: 'changed' },
    { ...command, target: { ...command.target, incarnation: 'replacement' } },
    { action: 'retire' as const, target: command.target },
  ]) {
    expect(() => applyCommandStatus(records, 'run-1', { operationId, command: altered, state: 'input_admitted', envelopeId: 7, receipt: null })).toThrow(/exact target and contents/)
  }
  expect(records[0]?.state).toBe('queued')
})

it('allocates an HTTP-compatible UUID without crypto.randomUUID', () => {
  const randomValues = crypto.getRandomValues.bind(crypto)
  vi.stubGlobal('crypto', { getRandomValues: randomValues })
  try {
    expect(allocateOperationId()).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
  } finally { vi.unstubAllGlobals() }
})
