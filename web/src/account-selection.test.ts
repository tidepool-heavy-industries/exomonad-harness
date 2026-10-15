import { expect, it } from 'vitest'
import { applyStateEvent, normalizeSnapshot, sameHostCommand, isEmbeddedCommandRecord, type HostCommand, type Snapshot } from './protocol'
import { applyCommandReceipt, applyCommandStatus, isSettledCommand, needsCommandLookup, readPendingCommands, retainCommand, writePendingCommands } from './pending-commands'

const target = { run: 'run-1', actor: '/root', incarnation: '1' }
const operation_id = '11111111-1111-4111-8111-111111111111'
const command: HostCommand = { action: 'select_account', target, account_id: 'one' }
const submission = { operation_id, command }
const receipt = { commandId: operation_id, target, outcome: 'account_selected' as const, accountId: 'one' }

it('preserves run selection through snapshot normalization and ordered updates', () => {
  const selection = { selectedId: 'one', accounts: [{ id: 'one', label: 'One' }, { id: 'two', label: 'Two' }] }
  const snapshot: Snapshot = { seq: '0', hostRun: target.run, accountSelection: selection, conversations: [], requests: [], jobs: [], envelopes: [] }
  const initial = normalizeSnapshot(snapshot)
  expect(initial.accountSelection).toBe(selection)
  const value = { ...selection, selectedId: 'two' }
  const next = applyStateEvent(initial, { seq: '1', event: { kind: 'account_selection.upsert', value } })
  expect(next.kind === 'applied' && next.state.accountSelection).toBe(value)
  expect(initial.accountSelection?.selectedId).toBe('one')
  expect(applyStateEvent(initial, { seq: '2', event: { kind: 'account_selection.upsert', value } }).kind).toBe('resync')
  const otherRun = applyStateEvent(initial, { seq: '1', event: { kind: 'host_run.upsert', value: { run: 'another-run' } } })
  expect(otherRun.kind === 'applied' && otherRun.state.accountSelection).toBeUndefined()
})

it('treats the selected ID as immutable command contents and refuses mismatched receipts', () => {
  const edited = { ...command, account_id: 'two' }
  expect(sameHostCommand(command, edited)).toBe(false)
  const retained = retainCommand([], target.run, submission)
  expect(() => retainCommand(retained, target.run, { operation_id, command: edited })).toThrow()
  expect(() => applyCommandStatus(retained, target.run, { operationId: operation_id, command: edited, state: 'queued', envelopeId: null, receipt: null })).toThrow()
  expect(applyCommandReceipt(retained, { ...receipt, accountId: 'two' })).toEqual(retained)
  expect(isEmbeddedCommandRecord({ operationId: operation_id, command, state: 'account_selected', envelopeId: null, receipt: { ...receipt, accountId: 'two' } })).toBe(false)
})

it('settles only a matching authoritative account receipt and restores it without retry lookup', () => {
  sessionStorage.clear()
  const settled = applyCommandReceipt(retainCommand([], target.run, submission), receipt)
  expect(settled[0]?.state).toBe('account_selected')
  expect(isSettledCommand(settled[0]!)).toBe(true)
  expect(needsCommandLookup(settled[0]!)).toBe(false)
  writePendingCommands(settled)
  expect(readPendingCommands()).toEqual(settled)
  sessionStorage.clear()
})
