import { beforeEach, describe, expect, it, vi } from 'vitest'
import { allocateOperationId, applyCommandReceipt, applyCommandStatus, isSettledCommand, needsCommandLookup, observeCommand, readCommandLedger, readPendingCommands, retainCommand, writePendingCommands } from './pending-commands'

beforeEach(() => sessionStorage.clear())

const command = { action: 'input' as const, target: { run: 'run-1', actor: '/root', incarnation: 'inc-1' }, text: 'continue' }

describe('retained embedded browser operations', () => {
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
    expect(next).toMatchObject([{ ...retained[0]!, authority: 'status', state: 'input_admitted', envelopeId: 42 }])
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
  expect(readPendingCommands()).toEqual([{ ...valid, authority: 'legacy' }])
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

const operation_id = 'ABCDEF01-1111-4111-8111-111111111111'
const admittedReceipt = { commandId: operation_id.toLowerCase(), outcome: 'admitted' as const, target: command.target, envelopeId: '42' }

it('preserves valid legacy evidence and malformed original bytes during v2 migration', () => {
  sessionStorage.clear()
  const valid = { hostRun: 'run-1', submission: { operation_id, command }, state: 'input_admitted', receipt: admittedReceipt }
  const original = JSON.stringify([valid, { broken: ' exact λ ' }])
  sessionStorage.setItem('harness.embeddedCommands.v1', original)
  const migrated = readCommandLedger()
  expect(migrated.records).toMatchObject([{ ...valid, authority: 'legacy' }])
  expect(isSettledCommand(migrated.records[0]!)).toBe(false)
  expect(needsCommandLookup(migrated.records[0]!)).toBe(true)
  writePendingCommands(migrated.records)
  expect(sessionStorage.getItem('harness.embeddedCommands.v1')).toBe(original)
  expect(readCommandLedger().quarantine.some((entry) => entry.raw.includes(' exact λ '))).toBe(true)
  sessionStorage.setItem('harness.embeddedCommands.v2', ' malformed λ ')
  writePendingCommands(readPendingCommands())
  expect(readCommandLedger().quarantine.some((entry) => entry.raw === ' malformed λ ')).toBe(true)
})

it('joins canonical UUID receipts only with exact supplied targets and preserves terminal handoffs', () => {
  const records = retainCommand([], 'run-1', { operation_id, command })
  expect(records[0]?.authority).toBe('local')
  expect(applyCommandReceipt(records, { ...admittedReceipt, target: undefined })).toEqual(records)
  expect(applyCommandReceipt(records, { ...admittedReceipt, target: { ...command.target, incarnation: 'INC-1' } })).toEqual(records)
  const joined = applyCommandReceipt(records, admittedReceipt)
  expect(joined[0]).toMatchObject({ authority: 'receipt', state: 'input_admitted', receipt: admittedReceipt })
  expect(joined[0]?.submission.operation_id).toBe(operation_id)
  expect(applyCommandStatus(joined, 'run-1', { operationId: operation_id.toLowerCase(), command, state: 'queued', envelopeId: null, receipt: null })).toEqual(joined)
})

it('enriches admitted wake observations and retains contradictory terminal evidence visibly', () => {
  const admitted = applyCommandReceipt(retainCommand([], 'run-1', { operation_id, command }), admittedReceipt)
  const enriched = applyCommandReceipt(admitted, { ...admittedReceipt, wakeError: 'wake unavailable' })
  expect(enriched[0]?.receipt).toEqual({ ...admittedReceipt, wakeError: 'wake unavailable' })
  expect(applyCommandReceipt(enriched, admittedReceipt)[0]?.receipt).toEqual(enriched[0]?.receipt)
  const contradiction = applyCommandReceipt(enriched, { commandId: operation_id, target: command.target, outcome: 'refused', reason: 'denied' })
  expect(contradiction[0]?.state).toBe('input_admitted')
  expect(contradiction[0]?.issues).toEqual(expect.arrayContaining([expect.stringMatching(/Contradictory handoff/)]))
})

it('keeps local channel refusal and 404 separate from Store unconfirmed, which stops polling without eviction', () => {
  const observed = observeCommand(retainCommand([], 'run-1', { operation_id, command }), 'run-1', operation_id, {
    accepted: true, send: 'unknown', localRefusal: { operation_id, code: 'unavailable', reason: 'channel unavailable' },
    lookup: { kind: 'unavailable', reason: 'HTTP 404' },
  })
  expect(observed[0]).toMatchObject({ authority: 'local', state: 'queued', accepted: true, send: 'unknown' })
  expect(needsCommandLookup(observed[0]!)).toBe(true)
  const unconfirmed = applyCommandStatus(observed, 'run-1', { operationId: operation_id, command,
    state: 'unconfirmed', envelopeId: null, receipt: { commandId: operation_id, target: command.target, outcome: 'unconfirmed', reason: 'ack lost' } })
  expect(needsCommandLookup(unconfirmed[0]!)).toBe(false)
  expect(isSettledCommand(unconfirmed[0]!)).toBe(false)
})

it('caps resolved history globally without resurrecting v1 or evicting unresolved operations across runs', () => {
  sessionStorage.clear()
  const unresolved = retainCommand([], 'run-1', { operation_id: '00000000-0000-4000-8000-000000000000', command })
  const old = retainCommand([], 'old-run', { operation_id: '00000000-0000-4000-8000-000000000001', command: { ...command, target: { ...command.target, run: 'old-run' } } })
  const settled = Array.from({ length: 140 }, (_, index) => ({ ...unresolved[0]!, authority: 'status' as const, state: 'refused' as const,
    submission: { operation_id: `00000000-0000-4000-8000-${String(index + 2).padStart(12, '0')}`, command: { ...command, target: { ...command.target, run: index % 2 ? 'run-1' : 'old-run' } } }, hostRun: index % 2 ? 'run-1' : 'old-run' }))
  sessionStorage.setItem('harness.embeddedCommands.v1', JSON.stringify(settled))
  writePendingCommands([...old, ...unresolved, ...settled])
  const restored = readPendingCommands()
  expect(restored).toHaveLength(130)
  expect(restored).toContainEqual(old[0])
  expect(restored).toContainEqual(unresolved[0])
  expect(restored.filter(isSettledCommand)).toHaveLength(128)
})

it('preserves original UUID spelling and immutable payload while comparing round UUIDs canonically', () => {
  const round = 'ABCDEF02-1111-4111-8111-111111111111'
  const interrupt = { action: 'interrupt' as const, target: command.target, expected_round: round }
  const records = retainCommand([], 'run-1', { operation_id, command: interrupt })
  expect(retainCommand(records, 'run-1', { operation_id: operation_id.toLowerCase(), command: { ...interrupt, expected_round: round.toLowerCase() } })).toHaveLength(1)
  expect(records[0]?.submission.command).toEqual(interrupt)
  expect(Object.isFrozen(records[0]?.submission.command.target)).toBe(true)
  expect(() => retainCommand(records, 'run-1', { operation_id, command: { ...interrupt, target: { ...command.target, actor: '/ROOT' } } })).toThrow(/different contents/)
})

it('blocks writing when original storage is unreadable and preserves unseen records after recovery', () => {
  const prior = retainCommand([], 'run-1', { operation_id, command })
  writePendingCommands(prior)
  const original = sessionStorage.getItem('harness.embeddedCommands.v2')
  const reader = vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('read denied') })
  expect(readCommandLedger().available).toBe(false)
  expect(() => writePendingCommands([])).toThrow(/existing data was preserved/)
  reader.mockRestore()
  expect(sessionStorage.getItem('harness.embeddedCommands.v2')).toBe(original)
  writePendingCommands([])
  expect(readPendingCommands()).toEqual(prior)
})

it('quarantines persisted authoritative receipts that contradict immutable target, action, UUID or envelope', () => {
  const local = retainCommand([], 'run-1', { operation_id, command })[0]!
  for (const changes of [
    { state: 'control_requested', receipt: { ...admittedReceipt, outcome: 'control_requested', control: 'retire' } },
    { receipt: { ...admittedReceipt, commandId: '00000000-0000-4000-8000-000000000000' } },
    { receipt: { ...admittedReceipt, target: { ...command.target, incarnation: 'OTHER' } } },
    { envelopeId: 43 },
  ]) {
    sessionStorage.setItem('harness.embeddedCommands.v2', JSON.stringify({ version: 2, records: [
      { ...local, authority: 'receipt', state: 'input_admitted', receipt: admittedReceipt, ...changes },
    ] }))
    expect(readCommandLedger().records).toHaveLength(0)
    expect(readCommandLedger().quarantine).toHaveLength(1)
  }
})
